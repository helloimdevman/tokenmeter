//! 파일 찾기, 폴, JSON·JSONL 읽기. 소스(F11)마다 파일 상태, 서비스마다 장부·프로브 캐시.

use super::checkpoint::{self, FileEntry, ReaderFile};
use super::delta::TokenDelta;
use super::expr::FileVars;
use super::ledger::{fnv1a64, Ledger, Vals};
use super::now_secs;
use super::roots::{dedup, excluded, expand, glob_under, read_outside, roots_from, Vars};
use super::spec::{Compiled, ServiceSpec};
use super::sqlite::{self, DbStamp, SqlErr};
use glob::{MatchOptions, Pattern};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(super) const TOKEN_FIELDS: [&str; 4] = ["input", "cache_read", "cache_write", "output"];
/// 이 안에 바뀐 파일은 뜨거운 항목: 처음 실행에서 끝까지 배우고, 내보낼 때 전체를 둔다(스펙 4.1, 4.3).
const HOT_SECS: f64 = 48.0 * 3600.0;
/// 밀린 기록의 상한: 이보다 이른 레코드는 배우기만 한다(스펙 4.4).
pub(super) const BACKLOG_SECS: f64 = 7.0 * 86_400.0;
/// `head`는 파일 앞 이만큼의 해시다.
const HEAD_BYTES: u64 = 4096;
/// 서비스 하나의 키 장부 상한(스펙 F2).
const LEDGER_CAP: usize = 500_000;
/// 같은 DB를 이보다 자주 쿼리하지 않는다(스펙 5절).
const SQLITE_GAP_SECS: f64 = 2.0;
/// 커서 없는 쿼리가 이보다 많은 행을 내면 `doctor` 경고.
const SQLITE_BIG_SCAN: usize = 10_000;
/// 이 안에 바뀐 파일(과 그 부모 디렉터리)만 폴마다 stat한다(스펙 14절 폴 설계).
const HOT_POLL_SECS: f64 = 3600.0;
/// 전체 glob과 차가운 파일 stat 간격.
const SCAN_SECS: f64 = 60.0;
/// 파일 하나를 폴 한 번에 읽는 상한과 JSONL 조각 크기(스펙 14절).
pub(super) const POLL_BYTES: u64 = 32 << 20;
const CHUNK_BYTES: usize = 1 << 20;
/// 이보다 큰 JSON 통파일은 mtime이 `SETTLE_SECS` 그대로일 때, 계속 바뀌면 늦어도 `SETTLE_MAX_SECS`마다 읽는다.
const BIG_JSON: u64 = 1_000_000;
const SETTLE_SECS: f64 = 2.0;
const SETTLE_MAX_SECS: f64 = 10.0;

/// 읽기 옵션. `gate`는 시각 문턱(스펙 4.4)이다. None이면 문턱과 7일 상한이 없다.
#[derive(Clone, Copy, Debug, Default)]
pub struct ReadOpts {
    pub gate: Option<f64>,
}

impl ReadOpts {
    /// 하네스와 `doctor --since`: 모든 레코드가 새것이다. `replay_gate`와 키 장부는 그대로 쓴다.
    pub fn no_gate() -> Self {
        Self { gate: None }
    }
}

/// 파일 한 번 읽기의 레코드 처리 방식.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pass {
    /// 키·기준값·문맥만 배운다(처음 실행, 차가운 항목의 `off`까지).
    Learn,
    /// 아는 파일에 새로 붙은 줄: 모두 새 기록이다(7일 상한과 새 누적 스트림만 문턱을 본다).
    Known,
    /// 모르는 파일, JSON 통파일, SQLite 행: 장부에 없는 레코드는 문턱으로 가른다.
    Unknown,
}

pub struct ServiceReader {
    pub spec: ServiceSpec,
    pub(super) opts: ReadOpts,
    /// 서비스 수준 자리(프로브 키, `live_chars`). 소스 자리는 `Source::x`.
    pub(super) x: Compiled,
    pub(super) sources: Vec<Source>,
    /// 벤더 → 요금제 라벨(F9).
    pub(super) plan: HashMap<String, String>,
    pub(super) endpoint: HashMap<String, String>,
    /// delta 키(없으면 레코드 해시)의 칸별 최댓값. 서비스에 하나, 소스들이 같이 쓴다(F2, F11).
    pub(super) ledger: Ledger,
    pub(super) live_out: HashMap<String, i64>,
    /// 읽은 레코드 수와 필드마다 값이 잡힌 수(`doctor`).
    pub stats: ReadStats,
    /// 마지막 전체 훑기 시각, 다음 폴에 훑으라는 요청(`SessionStart`), 마지막 `poll_at`이 전체 훑기였나.
    scanned: f64,
    scan_asked: bool,
    pub last_full: bool,
    /// JSONL 파일 하나를 한 번에 읽는 상한. `doctor --since`는 끝까지 읽는다.
    cap: u64,
}

/// `doctor`가 보이는 읽기 수. `hits`는 `TOKEN_FIELDS` 순서로 match를 지난 레코드 중 값이 잡힌 수.
#[derive(Clone, Debug, Default)]
pub struct ReadStats {
    pub records: u64,
    pub dropped_by_match: u64,
    pub matched: u64,
    pub hits: [u64; 4],
    /// SQLite: `SQLITE_BUSY`가 세 번 이어진 횟수, 모든 쿼리가 실패한 횟수, 커서 없이 1만 행을 넘은 DB 수.
    pub sqlite_busy: u64,
    pub sqlite_failed: u64,
    pub sqlite_big_scans: u64,
    /// 폴이 한 파일·디렉터리 stat 수(폴 설계 확인용).
    pub stat_calls: u64,
}

/// SQLite DB 하나의 읽기 상태(F1). 커서는 결과 열 값의 단위 그대로다.
#[derive(Default)]
pub(super) struct DbState {
    pub(super) stamp: DbStamp,
    pub(super) cursor: Option<i64>,
    /// 마지막 쿼리 시각(유닉스 초).
    pub(super) queried: f64,
    busy: u32,
    big: bool,
}

/// 읽기 단위 하나(F11): 병합한 스펙과 그 파일 상태.
#[derive(Default)]
pub(super) struct Source {
    pub(super) spec: ServiceSpec,
    pub(super) x: Compiled,
    pub(super) offset: HashMap<String, u64>,
    mtime: HashMap<String, f64>,
    /// 마지막으로 읽을 때의 (ino, 크기).
    stat: HashMap<String, (u64, u64)>,
    /// JSONL 앞 min(4096, off) 바이트의 (FNV-1a, 길이). 다시 쓰인 파일을 알아본다(스펙 4.1).
    pub(super) head: HashMap<String, (u64, u32)>,
    pub(super) ctx: HashMap<String, HashMap<String, String>>,
    /// cumulative 기준값: 파일 → 스트림 키 해시(키가 없으면 None) → 값.
    pub(super) base: HashMap<String, HashMap<Option<u64>, Vals>>,
    /// 파일마다 지난 `rebase_on` 값의 해시.
    pub(super) roll: HashMap<String, u64>,
    /// 이번 파일 읽기에서 `rebase_on`이 바뀌었다: 기준값만 잡고 내지 않는다.
    pub(super) rolling: bool,
    /// 이번에 읽는 파일의 mtime(SQLite는 `-wal`까지 본 최신). 시각 없는 레코드가 문턱과 견준다(4.4).
    pub(super) file_mtime: f64,
    /// 차가운 항목(읽지 않은 blind 포함): 바뀌면 0부터 `off`까지 배우며 읽고 그 뒤를 낸다(B2).
    pub(super) blind: HashSet<String>,
    /// 파일마다 처음 파싱된 레코드 시각(match와 상관없이, 스펙 4.5).
    pub(super) first: HashMap<String, f64>,
    /// `format: sqlite`의 DB마다 도장·커서.
    pub(super) db: HashMap<String, DbState>,
    /// 사이드카 경로 → (mtime, 파싱한 값). mtime이 바뀌면 다시 읽는다(F6).
    side: HashMap<PathBuf, (f64, Option<Value>)>,
    /// 뜨거운 파일의 부모 디렉터리 → 마지막으로 본 mtime.
    dirs: HashMap<PathBuf, f64>,
    /// 마지막 훑기 때 루트 → mtime(없으면 None).
    root_dirs: Vec<(PathBuf, Option<f64>)>,
    /// 다시 쓰이는 큰 JSON 통파일 → (지난번 읽은 판의 mtime, 마지막 본 mtime, 그 mtime을 본 시각).
    settle: HashMap<String, (f64, f64, f64)>,
}

/// 파일 하나를 읽는 동안의 자리: `$file.*`, `$root`, 사이드카 값(F6).
pub(super) struct FileCtx {
    pub(super) key: String,
    pub(super) vars: FileVars,
    pub(super) root: Option<String>,
    pub(super) side: HashMap<String, Value>,
}

impl Source {
    fn new(spec: ServiceSpec) -> Self {
        // 로더가 이미 검증했다. 검증 없이 만든 스펙(adapter check)의 틀린 식은 아무것도 읽지 않는다.
        let x = Compiled::new(&spec).unwrap_or_default();
        Self { spec, x, ..Default::default() }
    }

    /// 루트 틀을 펴고(B4: 못 펴면 버림) 정규화한 경로로 합친 것과 루트마다의 패턴(F10).
    /// ponytail: `roots_from` 레지스트리(1 MB 상한)를 훑을 때마다 다시 읽는다. 폴 비용이 보이면 mtime 캐시(2.11).
    fn roots(&self) -> Vec<(PathBuf, Vec<String>)> {
        let vars = Vars { root: None, ctx: &|_| None };
        let fixed = self.spec.roots.iter().filter_map(|r| expand(r, &vars).ok().flatten()).flatten();
        let mut roots: Vec<(PathBuf, Vec<String>)> =
            dedup(fixed.collect()).into_iter().map(|r| (r, self.spec.patterns.clone())).collect();
        for rf in &self.x.roots_from {
            roots.extend(roots_from(rf, &vars).0);
        }
        roots
    }

    /// 파일 하나를 읽을 자리. 루트는 그 파일을 품은 가장 깊은 루트다.
    /// 사이드카는 읽을 때마다 stat하고 mtime이 같으면 캐시한 값을 쓴다(스펙 2.0).
    /// ponytail: 루트를 찾으려고 읽을 때마다 루트 틀을 다시 편다. 폴 비용이 보이면 `files()`가 루트를 적어 둔다.
    fn file_ctx(&mut self, path: &Path) -> FileCtx {
        let root = self
            .roots()
            .into_iter()
            .map(|(r, _)| r)
            .filter(|r| path.starts_with(r))
            .max_by_key(|r| r.as_os_str().len());
        let dir = path.parent().unwrap_or(Path::new(""));
        let mut side = HashMap::new();
        let vars = Vars { root: root.as_deref(), ctx: &|_| None };
        for (name, template) in &self.x.sidecars {
            // 상대 경로는 데이터 파일 기준, 절대 경로(`~`, `$root`, 환경 변수)는 그대로
            let Some(p) = expand(template, &vars).ok().flatten().and_then(|v| v.into_iter().next()) else {
                continue;
            };
            let p = dir.join(p);
            let Ok(stat) = fs::metadata(&p) else { continue };
            let mtime = mtime_of(&stat);
            let value = match self.side.get(&p) {
                Some((m, v)) if *m == mtime => v.clone(),
                _ => {
                    if self.side.len() > 10_000 {
                        self.side.clear();
                    }
                    let v = read_outside(&p);
                    self.side.insert(p, (mtime, v.clone()));
                    v
                }
            };
            if let Some(v) = value {
                side.insert(name.clone(), v);
            }
        }
        let name = |s: Option<&std::ffi::OsStr>| s.map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        FileCtx {
            key: path_key(path),
            vars: FileVars {
                path: path_key(path),
                name: name(path.file_name()),
                stem: name(path.file_stem()),
                dirs: dir
                    .components()
                    .filter_map(|c| match c {
                        std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                        _ => None,
                    })
                    .collect(),
            },
            root: root.map(|r| r.to_string_lossy().into_owned()),
            side,
        }
    }

    fn files(&self) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (root, patterns) in self.roots() {
            // 없는 루트는 stat 한 번
            if !root.is_dir() {
                continue;
            }
            for pattern in &patterns {
                for p in glob_under(&root, pattern) {
                    if !excluded(&root, &p, &self.x.exclude) && seen.insert(p.clone()) {
                        out.push(p);
                    }
                }
            }
        }
        out
    }

    /// 지난 읽기 뒤로 바뀌었고 지금 읽을 때다. 1 MB 넘는 JSON 통파일은 다시 쓰기가 멎을 때까지 미룬다.
    fn due(&mut self, path: &Path, now: f64, stats: &mut u64) -> bool {
        *stats += 1;
        let key = path_key(path);
        if self.spec.format == "sqlite" {
            // 커밋은 `-wal`에 쌓이므로 본 파일 mtime만으로는 놓친다(스펙 5절)
            return !self.db.get(&key).is_some_and(|d| sqlite::stamp(path) == Some(d.stamp));
        }
        let Ok(stat) = fs::metadata(path) else { return false };
        let mtime = mtime_of(&stat);
        let known = self.mtime.get(&key).copied();
        if known == Some(mtime)
            && (self.spec.format == "json" || self.offset.get(&key).copied().unwrap_or(0) >= stat.len())
        {
            return false;
        }
        let Some(read) = known.filter(|_| self.spec.format == "json" && stat.len() > BIG_JSON) else {
            return true;
        };
        // 10초 상한은 지난번에 읽은 판의 mtime부터 잰다
        let (first, seen, seen_at) = *self.settle.entry(key.clone()).or_insert((read.min(now), mtime, now));
        if seen != mtime {
            self.settle.insert(key.clone(), (first, mtime, now));
        }
        let ready = (seen == mtime && now - seen_at >= SETTLE_SECS) || now - first >= SETTLE_MAX_SECS;
        if ready {
            self.settle.remove(&key);
        }
        ready
    }

    /// 뜨거운 집합: 1시간 안에 바뀌었거나 아직 끝까지 읽지 못한 파일.
    fn hot(&self, now: f64) -> Vec<PathBuf> {
        let recent = |t: f64| now - t <= HOT_POLL_SECS;
        let files = self.mtime.iter().filter(|(k, m)| {
            recent(**m) || self.offset.get(*k).is_some_and(|off| self.stat.get(*k).is_some_and(|s| *off < s.1))
        });
        let dbs = self.db.iter().filter(|(_, d)| recent(d.stamp.mtime.max(d.stamp.wal_mtime)));
        files.map(|(k, _)| k).chain(dbs.map(|(k, _)| k)).map(PathBuf::from).collect()
    }

    /// 디렉터리 하나에서 이 소스가 찾을 아직 모르는 파일(루트 패턴과 `exclude`를 그대로 따른다).
    fn new_in(&self, dir: &Path) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(dir) else { return Vec::new() };
        let roots = self.roots();
        let opts = MatchOptions { require_literal_separator: true, ..MatchOptions::new() };
        let wanted = |p: &Path| {
            roots.iter().any(|(root, pats)| {
                p.strip_prefix(root).is_ok_and(|rel| {
                    pats.iter().any(|pat| Pattern::new(pat).is_ok_and(|g| g.matches_path_with(rel, opts)))
                }) && !excluded(root, p, &self.x.exclude)
            })
        };
        entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let k = path_key(p);
                !self.mtime.contains_key(&k) && !self.db.contains_key(&k)
            })
            .filter(|p| p.is_file() && wanted(p))
            .collect()
    }
}

impl ServiceReader {
    /// 문턱 없이(`ReadOpts::no_gate`). 데몬은 `prime`이나 `set_gate`로 문턱을 둔다.
    pub fn new(spec: ServiceSpec) -> Self {
        Self::with_opts(spec, ReadOpts::no_gate())
    }

    pub fn with_opts(spec: ServiceSpec, opts: ReadOpts) -> Self {
        let x = Compiled::new(&spec).unwrap_or_default();
        // 소스가 없으면 서비스 자체가 소스 하나다(F11)
        let views = if spec.sources.is_empty() {
            vec![spec.clone()]
        } else {
            spec.sources.clone()
        };
        Self {
            spec,
            opts,
            x,
            sources: views.into_iter().map(Source::new).collect(),
            plan: HashMap::new(),
            endpoint: HashMap::new(),
            ledger: Ledger::new(now_secs, LEDGER_CAP),
            live_out: HashMap::new(),
            stats: ReadStats::default(),
            scanned: f64::NEG_INFINITY,
            scan_asked: false,
            last_full: false,
            cap: POLL_BYTES,
        }
    }

    /// 어느 소스든 경로가 적힌 필드와 값이 잡힌 비율(match를 지난 레코드 기준).
    pub fn field_hits(&self) -> Vec<(&'static str, f64)> {
        let matched = self.stats.matched.max(1) as f64;
        TOKEN_FIELDS
            .iter()
            .enumerate()
            .filter(|(i, _)| self.sources.iter().any(|s| s.x.fields[*i].is_some()))
            .map(|(i, name)| (*name, self.stats.hits[i] as f64 / matched))
            .collect()
    }

    /// `since`(유닉스 초) 뒤에 바뀐 파일을 처음부터 읽어 모든 델타를 낸다. 문턱 없음(`doctor --since`).
    /// 새 읽기 도구에서 부르므로 모든 키가 "지금 본 것"으로 적힌다(스펙 4.4).
    pub fn read_since(&mut self, since: f64) -> Vec<TokenDelta> {
        self.cap = u64::MAX;
        let mut all = Vec::new();
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| {
                for path in src.files() {
                    let changed = if src.spec.format == "sqlite" {
                        sqlite::stamp(&path).is_some_and(|s| s.mtime.max(s.wal_mtime) >= since)
                    } else {
                        fs::metadata(&path).is_ok_and(|m| mtime_of(&m) >= since)
                    };
                    if changed {
                        all.extend(me.read_in(src, &path, Pass::Unknown));
                    }
                }
            });
        }
        self.cap = POLL_BYTES;
        all
    }

    /// 모든 소스가 찾는 파일. 두 소스가 같은 파일을 읽어도 한 번만 든다.
    pub fn files(&self) -> Vec<PathBuf> {
        let mut seen = HashSet::new();
        let mut out: Vec<PathBuf> = self.sources.iter().flat_map(Source::files).collect();
        out.retain(|p| seen.insert(p.clone()));
        out
    }

    /// 소스 `i`를 잠시 꺼내 읽는다. 소스는 제 파일 상태를, 장부·프로브 캐시는 `self`를 쓴다.
    fn with_source(&mut self, i: usize, f: impl FnOnce(&mut Self, &mut Source)) {
        let mut src = std::mem::take(&mut self.sources[i]);
        f(self, &mut src);
        self.sources[i] = src;
    }

    /// 시각 문턱(스펙 4.4). 데몬이 max(마지막 커밋 − 10분, 지금 − 7일, 측정을 켠 시각)을 넣는다.
    pub fn set_gate(&mut self, threshold: f64) {
        self.opts.gate = Some(threshold);
    }

    /// 데몬의 처음 실행: 문턱은 지금(스펙 4.3).
    pub fn prime(&mut self) {
        self.set_gate(now_secs());
        self.restore(None, Vec::new());
    }

    /// 저장한 읽기 상태를 되살린다. None이면 처음 실행이다(스펙 4.3): 48시간 안의 JSONL은 끝까지,
    /// JSON 통파일은 나이와 상관없이 한 번, SQLite는 커서 없이 끝까지 읽어 배우기만 하고,
    /// 오래된 JSONL은 끝 위치의 차가운 항목으로 둔다. 루트를 다시 훑어 해시로 항목을 찾고,
    /// 못 찾은 항목(지워진 파일)은 버린다. 시각 문턱은 건드리지 않는다.
    pub fn restore(&mut self, file: Option<ReaderFile>, keys: Vec<(u64, [f64; 7])>) {
        self.ledger.load(keys);
        let first_run = file.is_none();
        let mut entries = file.map(|f| f.files).unwrap_or_default();
        let now = now_secs();
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| {
                for path in src.files() {
                    if first_run {
                        let Ok(stat) = fs::metadata(&path) else { continue };
                        if src.spec.format == "jsonl" && now - mtime_of(&stat) > HOT_SECS {
                            let key = path_key(&path);
                            src.offset.insert(key.clone(), stat.len());
                            src.mtime.insert(key.clone(), mtime_of(&stat));
                            src.stat.insert(key.clone(), (stat.ino(), stat.len()));
                            src.blind.insert(key);
                        } else {
                            let _ = me.read_in(src, &path, Pass::Learn);
                        }
                    } else if let Some(e) = entries.remove(&entry_key(i, &path)) {
                        src.load_entry(&path, e);
                    }
                }
            });
        }
    }

    /// 파일 상태와 마지막 내보내기 뒤 새로 생기거나 늘어난 키. `seq`는 부르는 쪽이 채운다(4.2).
    /// 48시간 넘게 바뀌지 않은 JSONL은 짧은 항목이다. 없어진 파일은 여기서 잊는다.
    pub fn export(&mut self) -> (ReaderFile, Vec<(u64, [f64; 7])>) {
        let now = now_secs();
        let mut files = std::collections::BTreeMap::new();
        for (i, src) in self.sources.iter_mut().enumerate() {
            let paths: HashSet<String> = src.mtime.keys().chain(src.db.keys()).cloned().collect();
            for p in paths {
                let path = PathBuf::from(&p);
                if !path.exists() {
                    src.forget(&p);
                    src.base.remove(&p);
                    src.roll.remove(&p);
                    src.db.remove(&p);
                    src.mtime.remove(&p);
                    src.stat.remove(&p);
                    continue;
                }
                files.insert(entry_key(i, &path), src.entry(&p, now));
            }
        }
        let file = ReaderFile { v: 1, seq: 0, at: now, files, est: Default::default() };
        (file, self.ledger.take_dirty())
    }

    /// 커밋 때만: 오래된 키를 버린다(F2). 데몬이 `export` 앞에서 부른다.
    pub fn prune_keys(&mut self) {
        self.ledger.prune();
    }

    /// `.keys` 압축(4.1)에 쓸 살아 있는 키.
    pub fn live_keys(&self) -> Vec<(u64, [f64; 7])> {
        self.ledger.live()
    }

    /// 다음 폴이 같은 DB 쿼리 간격(2초)을 기다리지 않게 한다. 하네스가 step-2 앞에서 부른다.
    #[doc(hidden)]
    pub fn forget_sqlite_gap(&mut self) {
        for s in &mut self.sources {
            s.db.values_mut().for_each(|d| d.queried = 0.0);
        }
    }

    /// 전체 훑기: glob으로 찾은 파일을 모두 stat해 바뀐 것을 읽는다(하네스와 테스트, 데몬의 60초 훑기).
    pub fn poll(&mut self) -> Vec<TokenDelta> {
        self.scan(now_secs())
    }

    /// 데몬의 폴(스펙 14절 폴 설계): 60초마다와 `request_scan` 뒤에는 전체 훑기,
    /// 그 사이에는 뜨거운 파일과 그 부모 디렉터리만 stat하고 바뀐 디렉터리만 다시 읽는다.
    pub fn poll_at(&mut self, now: f64) -> Vec<TokenDelta> {
        self.last_full = self.scan_asked || now - self.scanned >= SCAN_SECS;
        if self.last_full {
            self.scan_asked = false;
            self.scanned = now;
            return self.scan(now);
        }
        let mut all = Vec::new();
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| all.extend(me.poll_hot(src, now)));
        }
        all
    }

    /// 새 세션(`live/<서비스>__*.json`)이 보였다: 다음 폴에 전체를 훑는다(새 프로젝트 디렉터리).
    pub fn request_scan(&mut self) {
        self.scan_asked = true;
    }

    fn scan(&mut self, now: f64) -> Vec<TokenDelta> {
        let mut all = Vec::new();
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| all.extend(me.scan_source(src, now)));
        }
        all
    }

    /// 소스 하나를 훑는다. 루트 mtime을 glob 앞에 적어 그사이 생긴 디렉터리는 다음 폴이 본다.
    fn scan_source(&mut self, src: &mut Source, now: f64) -> Vec<TokenDelta> {
        src.root_dirs = src.roots().into_iter().map(|(r, _)| (r.clone(), dir_mtime(&r))).collect();
        self.stats.stat_calls += src.root_dirs.len() as u64;
        let mut out = Vec::new();
        for path in src.files() {
            if src.due(&path, now, &mut self.stats.stat_calls) {
                out.extend(self.read_in(src, &path, Pass::Unknown));
            }
        }
        out
    }

    fn poll_hot(&mut self, src: &mut Source, now: f64) -> Vec<TokenDelta> {
        // 루트 바로 아래가 바뀌었다(새 프로젝트 디렉터리, 없던 루트가 생김): 이 소스를 바로 훑는다
        self.stats.stat_calls += src.root_dirs.len() as u64;
        if src.root_dirs.iter().any(|(r, m)| dir_mtime(r) != *m) {
            self.last_full = true;
            return self.scan_source(src, now);
        }
        let mut out = Vec::new();
        let mut dirs = HashSet::new();
        for path in src.hot(now) {
            if let Some(d) = path.parent() {
                dirs.insert(d.to_path_buf());
            }
            if src.due(&path, now, &mut self.stats.stat_calls) {
                out.extend(self.read_in(src, &path, Pass::Unknown));
            }
        }
        src.dirs.retain(|d, _| dirs.contains(d));
        for dir in dirs {
            self.stats.stat_calls += 1;
            let Ok(stat) = fs::metadata(&dir) else { continue };
            let m = mtime_of(&stat);
            // 처음 보는 디렉터리도 한 번 읽는다: 훑기와 이 폴 사이에 생긴 파일
            if src.dirs.insert(dir.clone(), m) == Some(m) {
                continue;
            }
            for path in src.new_in(&dir) {
                out.extend(self.read_in(src, &path, Pass::Unknown));
            }
        }
        out
    }

    /// 그 파일을 찾는 소스마다 읽는다(`doctor` 표본).
    /// ponytail: 소스가 여럿이면 부를 때마다 소스별 glob을 돈다. 느려지면 패턴 대조로 바꾼다.
    pub fn read_file(&mut self, path: &Path, emit: bool) -> Vec<TokenDelta> {
        let only = self.sources.len() == 1;
        let mut out = Vec::new();
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| {
                if only || src.files().iter().any(|p| p == path) {
                    out.extend(me.read_in(src, path, if emit { Pass::Unknown } else { Pass::Learn }));
                }
            });
        }
        out
    }

    fn read_in(&mut self, src: &mut Source, path: &Path, pass: Pass) -> Vec<TokenDelta> {
        let mut out = Vec::new();
        src.rolling = false;
        src.file_mtime = if src.spec.format == "sqlite" {
            sqlite::stamp(path).map_or(0.0, |s| s.mtime.max(s.wal_mtime))
        } else {
            fs::metadata(path).map_or(0.0, |m| mtime_of(&m))
        };
        let file = src.file_ctx(path);
        if src.spec.format == "json" {
            self.read_json(src, path, &file, &mut out, pass);
        } else if src.spec.format == "sqlite" {
            self.read_sqlite(src, path, &file, &mut out, pass);
        } else {
            self.read_jsonl(src, path, &file, &mut out, pass);
        }
        out
    }

    fn read_json(&mut self, src: &mut Source, path: &Path, file: &FileCtx, out: &mut Vec<TokenDelta>, pass: Pass) {
        let Ok(stat) = fs::metadata(path) else { return };
        let Ok(raw) = fs::read_to_string(path) else {
            return;
        };
        let raw = raw.trim();
        if raw.is_empty() {
            return;
        }
        let Ok(obj) = serde_json::from_str::<Value>(raw) else {
            return;
        };
        self.handle(src, &obj, file, out, pass);
        let key = path_key(path);
        src.stat.insert(key.clone(), (stat.ino(), stat.len()));
        src.mtime.insert(key, mtime_of(&stat));
    }

    /// 폴 사이에 연결·트랜잭션을 들고 있지 않는다: 열고, 문장 하나를 끝까지 돌리고, 닫는다.
    fn read_sqlite(&mut self, src: &mut Source, path: &Path, file: &FileCtx, out: &mut Vec<TokenDelta>, pass: Pass) {
        let key = path_key(path);
        let Some(stamp) = sqlite::stamp(path) else { return };
        let now = now_secs();
        let mut st = src.db.remove(&key).unwrap_or_default();
        if now - st.queried < SQLITE_GAP_SECS {
            src.db.insert(key, st);
            return;
        }
        // 다른 파일이 되었거나 줄었다: 커서를 비우고 다시 읽는다(키 장부가 이미 센 행을 거른다)
        if st.stamp.ino != stamp.ino || stamp.size < st.stamp.size {
            st.cursor = None;
        }
        st.queried = now;
        let cursor = Some(src.spec.cursor.as_str()).filter(|c| !c.is_empty());
        let result = sqlite::open_ro(path)
            .map_err(|e| SqlErr::Other(e.to_string()))
            .and_then(|conn| sqlite::run(&conn, &src.spec.query, cursor, st.cursor));
        match result {
            Ok((rows, max)) => {
                st.busy = 0;
                if cursor.is_none() && rows.len() > SQLITE_BIG_SCAN && !st.big {
                    st.big = true;
                    self.stats.sqlite_big_scans += 1;
                }
                for row in &rows {
                    // 행 사이에는 문맥을 잇지 않는다(파일 문맥 기억 없음)
                    src.ctx.remove(&key);
                    self.handle(src, row, file, out, pass);
                }
                src.ctx.remove(&key);
                st.cursor = st.cursor.max(max);
            }
            // 이번 폴을 건너뛴다. 도장을 두지 않아 다음 폴에 다시 한다.
            Err(SqlErr::Busy) => {
                st.busy += 1;
                if st.busy == 3 {
                    self.stats.sqlite_busy += 1;
                    eprintln!(
                        "[TokenMeter] {}",
                        crate::l10n!(
                            "{}: SQLite busy for 3 polls in a row",
                            "{}: SQLite가 폴 세 번 이어 바쁩니다",
                            self.spec.name
                        )
                    );
                }
                src.db.insert(key, st);
                return;
            }
            // 쿼리가 모두 실패했거나 열 수 없다: 다음 변화까지 건너뛴다
            Err(_) => self.stats.sqlite_failed += 1,
        }
        st.stamp = stamp;
        src.db.insert(key, st);
    }

    /// 아는 파일(`ino`·head 같음, `size ≥ off`)은 `off`부터 새 줄로, 차가운 항목은 `off`까지 배운 뒤 그 뒤를,
    /// 다시 쓰인 파일은 처음부터 모르는 파일로 읽는다(스펙 4.4).
    fn read_jsonl(&mut self, src: &mut Source, path: &Path, file: &FileCtx, out: &mut Vec<TokenDelta>, pass: Pass) {
        let Ok(stat) = fs::metadata(path) else { return };
        let key = path_key(path);
        let size = stat.len();
        let Ok(mut fh) = fs::File::open(path) else {
            return;
        };
        let (mut pass, mut start, mut learn_to) = (pass, 0, 0);
        if let Some(off) = src.offset.get(&key).copied().filter(|_| pass != Pass::Learn) {
            let same = src.stat.get(&key).is_none_or(|s| s.0 == stat.ino())
                && size >= off
                && src.head.get(&key).is_none_or(|h| head_of(&mut fh, u64::from(h.1)) == Some(*h));
            if !same {
                src.forget(&key);
            } else if src.blind.remove(&key) {
                (pass, learn_to) = (Pass::Known, off);
            } else {
                (pass, start) = (Pass::Known, off);
            }
        }
        src.stat.insert(key.clone(), (stat.ino(), size));
        if size == start {
            src.offset.insert(key.clone(), start);
            src.mtime.insert(key, mtime_of(&stat));
            return;
        }
        if fh.seek(SeekFrom::Start(start)).is_err() {
            return;
        }
        // 1 MiB 조각으로 줄 단위로 읽고, 배울 곳을 지나 폴당 `cap`까지만 낸다. 나머지는 다음 폴에.
        let limit = if pass == Pass::Learn { u64::MAX } else { start.max(learn_to).saturating_add(self.cap) };
        let mut rd = BufReader::with_capacity(CHUNK_BYTES, fh);
        let mut line = Vec::new();
        let mut pos = start;
        while pos < limit {
            line.clear();
            // 끝의 잘린 줄은 쓰지 않고 다음 폴까지 미룬다
            if !matches!(rd.read_until(b'\n', &mut line), Ok(n) if n > 0) || line.last() != Some(&b'\n') {
                break;
            }
            pos += line.len() as u64;
            let text = String::from_utf8_lossy(&line);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if let Ok(obj) = serde_json::from_str::<Value>(text) {
                let p = if pos <= learn_to { Pass::Learn } else { pass };
                self.handle(src, &obj, file, out, p);
            }
        }
        if pos == start {
            return;
        }
        let mut fh = rd.into_inner();
        if src.head.get(&key).is_none_or(|h| u64::from(h.1) < pos.min(HEAD_BYTES)) {
            if let Some(h) = head_of(&mut fh, pos.min(HEAD_BYTES)) {
                src.head.insert(key.clone(), h);
            }
        }
        src.offset.insert(key.clone(), pos);
        src.mtime.insert(key, mtime_of(&stat));
    }
}

impl Source {
    /// 다시 쓰인 파일: 위치·문맥·첫 시각·head를 잊는다. 기준값은 파일이 있는 동안 둔다(F2).
    fn forget(&mut self, key: &str) {
        self.offset.remove(key);
        self.ctx.remove(key);
        self.first.remove(key);
        self.head.remove(key);
        self.blind.remove(key);
    }

    /// 파일 하나의 저장 항목. 48시간 안에 바뀐 파일(SQLite는 늘)은 전체, 나머지는 짧은 항목(스펙 4.1).
    fn entry(&self, key: &str, now: f64) -> FileEntry {
        let hex = |h: u64| format!("{h:016x}");
        let (ino, size) = self.stat.get(key).copied().unwrap_or_default();
        let mut e = FileEntry {
            ino,
            size,
            mtime: self.mtime.get(key).copied().unwrap_or_default(),
            off: self.offset.get(key).copied().unwrap_or_default(),
            base: self
                .base
                .get(key)
                .map(|b| b.iter().map(|(k, v)| (k.map(hex).unwrap_or_default(), *v)).collect())
                .unwrap_or_default(),
            roll: self.roll.get(key).copied().map(hex),
            ..Default::default()
        };
        if let Some(d) = self.db.get(key) {
            (e.ino, e.size, e.mtime) = (d.stamp.ino, d.stamp.size, d.stamp.mtime);
            e.wal = Some((d.stamp.wal_size, d.stamp.wal_mtime));
            e.cursor = d.cursor;
        } else if !self.blind.contains(key) && now - e.mtime <= HOT_SECS {
            e.head = self.head.get(key).map(|(h, n)| (hex(*h), *n));
            e.first = self.first.get(key).copied();
            e.ctx = self.ctx.get(key).map(|c| c.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        }
        e
    }

    /// 저장 항목을 되살린다. head가 없는 JSONL 항목(차가운 항목)은 blind다.
    fn load_entry(&mut self, path: &Path, e: FileEntry) {
        let key = path_key(path);
        let unhex = |s: &str| u64::from_str_radix(s, 16).ok();
        if self.spec.format == "sqlite" {
            let (wal_size, wal_mtime) = e.wal.unwrap_or_default();
            let stamp = DbStamp { ino: e.ino, size: e.size, mtime: e.mtime, wal_size, wal_mtime };
            self.db.insert(key.clone(), DbState { stamp, cursor: e.cursor, ..Default::default() });
        } else {
            self.offset.insert(key.clone(), e.off);
            self.mtime.insert(key.clone(), e.mtime);
            self.stat.insert(key.clone(), (e.ino, e.size));
            match &e.head {
                Some((h, n)) => {
                    if let Some(h) = unhex(h) {
                        self.head.insert(key.clone(), (h, *n));
                    }
                }
                None if self.spec.format == "jsonl" && e.off > 0 => {
                    self.blind.insert(key.clone());
                }
                None => {}
            }
        }
        if let Some(t) = e.first {
            self.first.insert(key.clone(), t);
        }
        if !e.ctx.is_empty() {
            self.ctx.insert(key.clone(), e.ctx.into_iter().collect());
        }
        if !e.base.is_empty() {
            let base = e.base.iter().map(|(k, v)| ((!k.is_empty()).then(|| unhex(k)).flatten(), *v)).collect();
            self.base.insert(key.clone(), base);
        }
        if let Some(r) = e.roll.as_deref().and_then(unhex) {
            self.roll.insert(key, r);
        }
    }
}

/// 저장 항목 키: 경로 해시. 둘째 소스부터는 `<번호>.`을 붙여 같은 파일을 읽는 소스끼리 겹치지 않게 한다.
fn entry_key(source: usize, path: &Path) -> String {
    match source {
        0 => checkpoint::path_key(path),
        i => format!("{i}.{}", checkpoint::path_key(path)),
    }
}

/// 파일 앞 `len` 바이트의 (FNV-1a, 길이). 그만큼 읽지 못하면 None.
fn head_of(fh: &mut fs::File, len: u64) -> Option<(u64, u32)> {
    let mut buf = vec![0; len as usize];
    fh.seek(SeekFrom::Start(0)).ok()?;
    fh.read_exact(&mut buf).ok()?;
    Some((fnv1a64(&buf), len as u32))
}

pub(super) fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn dir_mtime(path: &Path) -> Option<f64> {
    fs::metadata(path).ok().map(|m| mtime_of(&m))
}

fn mtime_of(stat: &fs::Metadata) -> f64 {
    stat.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::specs_from_yaml;
    use std::time::Duration;

    /// 인라인 서비스 `t` 하나: 줄마다 `{"id", "out"}`.
    fn reader(root: &Path, body: &str) -> ServiceReader {
        let text = format!("services: {{t: {{roots: [{root:?}], key: id, fields: {{output: out}}{body}}}}}");
        ServiceReader::new(specs_from_yaml(&text).pop().unwrap())
    }

    /// `at` 유닉스 초를 mtime으로 두고 쓴다.
    fn write_at(path: &Path, text: &str, at: f64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        let f = fs::File::options().write(true).open(path).unwrap();
        f.set_modified(UNIX_EPOCH + Duration::from_secs_f64(at)).unwrap();
    }

    fn line(id: &str, out: i64) -> String {
        format!("{}\n", serde_json::json!({"id": id, "out": out}))
    }

    fn out(got: &[TokenDelta]) -> i64 {
        got.iter().map(|d| d.output_tokens).sum()
    }

    #[test]
    fn hot_set_is_stat_every_poll_and_cold_every_60s() {
        let (_g, tmp) = crate::test_home("hot-set");
        let t = now_secs();
        write_at(&tmp.join("d/p/hot.jsonl"), &line("h", 1), t - 10.0);
        for i in 0..3 {
            write_at(&tmp.join(format!("d/old{i}/c.jsonl")), &line(&format!("c{i}"), 1), t - 7200.0);
        }
        let mut r = reader(&tmp.join("d"), "");
        let stats = |r: &mut ServiceReader, now: f64| {
            let before = r.stats.stat_calls;
            r.poll_at(now);
            (r.last_full, r.stats.stat_calls - before)
        };
        assert_eq!(stats(&mut r, t), (true, 5), "처음 폴은 전체 훑기: 루트와 파일 넷");
        for k in 1..30 {
            // 루트, 뜨거운 파일 하나와 그 부모 디렉터리 하나
            assert_eq!(stats(&mut r, t + 2.0 * k as f64), (false, 3), "{k}");
        }
        assert_eq!(stats(&mut r, t + 60.0), (true, 5), "60초마다 차가운 파일까지");
        // 한 시간이 지나면 식는다
        assert_eq!(stats(&mut r, t + 3700.0), (true, 5));
        assert_eq!(stats(&mut r, t + 3702.0), (false, 1));
    }

    #[test]
    fn new_file_in_a_hot_parent_dir_is_found_next_poll() {
        let (_g, tmp) = crate::test_home("hot-dir");
        let t = now_secs();
        let dir = tmp.join("d/p");
        write_at(&dir.join("a.jsonl"), &line("a", 1), t);
        let mut r = reader(&tmp.join("d"), "");
        assert_eq!(out(&r.poll_at(t)), 1);
        assert!(r.poll_at(t + 2.0).is_empty());
        write_at(&dir.join("b.jsonl"), &line("b", 7), t + 3.0);
        write_at(&dir.join("skip.txt"), &line("x", 100), t + 3.0);
        let got = r.poll_at(t + 4.0);
        assert!(!r.last_full);
        assert_eq!(out(&got), 7, "같은 디렉터리의 새 세션, 패턴 밖 파일은 빼고");
        assert!(r.poll_at(t + 6.0).is_empty());
    }

    #[test]
    fn session_start_triggers_a_scan() {
        let (_g, tmp) = crate::test_home("scan-ask");
        let t = now_secs();
        write_at(&tmp.join("d/p/a.jsonl"), &line("a", 1), t);
        fs::create_dir_all(tmp.join("d/q/sub")).unwrap();
        let mut r = reader(&tmp.join("d"), "");
        r.poll_at(t);
        write_at(&tmp.join("d/q/sub/b.jsonl"), &line("b", 5), t + 1.0);
        assert!(r.poll_at(t + 2.0).is_empty(), "뜨겁지 않은 디렉터리의 새 파일은 뜨거운 폴이 모른다");
        r.request_scan();
        assert_eq!(out(&r.poll_at(t + 4.0)), 5);
        assert!(r.last_full);
        r.poll_at(t + 6.0);
        assert!(!r.last_full, "요청은 한 번만");
    }

    #[test]
    fn new_project_dir_under_a_root_is_found_next_poll() {
        let (_g, tmp) = crate::test_home("root-dir");
        let t = now_secs();
        let mut r = reader(&tmp.join("d"), "");
        assert!(r.poll_at(t).is_empty(), "루트가 아직 없다");
        write_at(&tmp.join("d/p/a.jsonl"), &line("a", 3), t + 1.0);
        assert_eq!(out(&r.poll_at(t + 2.0)), 3, "루트가 생겼다");
        write_at(&tmp.join("d/q/b.jsonl"), &line("b", 4), t + 3.0);
        assert_eq!(out(&r.poll_at(t + 4.0)), 4, "새 프로젝트 디렉터리");
        assert!(r.last_full);
    }

    #[test]
    fn large_jsonl_is_read_32mib_per_poll_on_line_boundaries() {
        assert_eq!(POLL_BYTES, 32 << 20);
        let (_g, tmp) = crate::test_home("chunks");
        let t = now_secs();
        let path = tmp.join("d/big.jsonl");
        let mut text: String = (0..100).map(|i| line(&format!("k{i:03}"), 1)).collect();
        let len = line("k000", 1).len() as u64;
        text.push_str("{\"id\": \"tail\", \"out\"");
        write_at(&path, &text, t);
        let mut r = reader(&tmp.join("d"), "");
        // 조각 상한을 줄 10.5개로: 폴마다 11줄(줄 경계까지), 잘린 끝 줄은 세지 않는다
        r.cap = len * 21 / 2;
        let mut seen = Vec::new();
        for k in 0..12 {
            seen.push(r.poll_at(t + 2.0 * k as f64).len());
            let off = r.sources[0].offset.get(&path_key(&path)).copied().unwrap_or(0);
            assert_eq!(off % len, 0, "줄 경계");
        }
        assert_eq!(seen, [11, 11, 11, 11, 11, 11, 11, 11, 11, 1, 0, 0]);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        std::io::Write::write_all(&mut f, b": 9}\n").unwrap();
        assert_eq!(out(&r.poll_at(t + 30.0)), 9, "끝 줄이 이어지면 다음 폴에");
    }

    #[test]
    fn big_json_rewritten_every_second_is_read_at_most_every_10s() {
        let (_g, tmp) = crate::test_home("settle");
        let t = now_secs();
        let path = tmp.join("d/ui.json");
        let pad = "x".repeat(1_100_000);
        let write = |i: usize| write_at(&path, &format!("{{\"id\": \"r{i}\", \"out\": 1, \"pad\": \"{pad}\"}}"), t + i as f64);
        write(0);
        let mut r = reader(&tmp.join("d"), ", format: json, patterns: [\"*.json\"]");
        let mut reads = vec![];
        for i in 0..=30 {
            write(i);
            if !r.poll_at(t + i as f64).is_empty() {
                reads.push(i);
            }
        }
        assert_eq!(reads, [0, 10, 20, 30], "처음 한 번, 그 뒤 계속 바뀌면 10초마다");
        // 쓰기가 멎으면 2초 뒤
        write(31);
        assert!(r.poll_at(t + 31.0).is_empty());
        assert!(r.poll_at(t + 32.0).is_empty());
        assert_eq!(out(&r.poll_at(t + 33.0)), 1);
    }
}
