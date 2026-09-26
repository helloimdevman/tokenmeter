//! 파일 찾기, 폴, JSON·JSONL 읽기. 소스(F11)마다 파일 상태, 서비스마다 장부·프로브 캐시.

use super::checkpoint::{self, FileEntry, ReaderFile};
use super::delta::TokenDelta;
use super::expr::FileVars;
use super::ledger::{fnv1a64, Ledger, Vals};
use super::now_secs;
use super::roots::{dedup, excluded, expand, glob_under, read_outside, roots_from, Vars};
use super::spec::{Compiled, ServiceSpec};
use super::sqlite::{self, DbStamp, SqlErr};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
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

    /// 지난 읽기 뒤로 바뀌지 않았다.
    fn unchanged(&self, path: &Path) -> bool {
        let key = path_key(path);
        if self.spec.format == "sqlite" {
            // 커밋은 `-wal`에 쌓이므로 본 파일 mtime만으로는 놓친다(스펙 5절)
            return self.db.get(&key).is_some_and(|d| sqlite::stamp(path) == Some(d.stamp));
        }
        fs::metadata(path).is_ok_and(|stat| {
            self.mtime.get(&key).copied() == Some(mtime_of(&stat))
                && (self.spec.format == "json"
                    || self.offset.get(&key).copied().unwrap_or(0) >= stat.len())
        })
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

    /// 다음 폴이 같은 DB 쿼리 간격(2초)을 기다리지 않게 한다. 하네스가 step-2 앞에서 부른다.
    #[doc(hidden)]
    pub fn forget_sqlite_gap(&mut self) {
        for s in &mut self.sources {
            s.db.values_mut().for_each(|d| d.queried = 0.0);
        }
    }

    pub fn poll(&mut self) -> Vec<TokenDelta> {
        let mut all = Vec::new();
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| {
                for path in src.files() {
                    if !src.unchanged(&path) {
                        all.extend(me.read_in(src, &path, Pass::Unknown));
                    }
                }
            });
        }
        all
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
        let mut buf = Vec::with_capacity((size - start).min(1024 * 1024) as usize);
        if fh.read_to_end(&mut buf).is_err() {
            return;
        }
        let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let mut pos = start;
        for chunk in buf[..end].split(|b| *b == b'\n') {
            pos += chunk.len() as u64 + 1;
            if chunk.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(chunk);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if let Ok(obj) = serde_json::from_str::<Value>(text) {
                let p = if pos <= learn_to { Pass::Learn } else { pass };
                self.handle(src, &obj, file, out, p);
            }
        }
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

fn mtime_of(stat: &fs::Metadata) -> f64 {
    stat.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
