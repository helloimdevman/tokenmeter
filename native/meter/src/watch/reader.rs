//! 파일 찾기, 폴, JSON·JSONL 읽기. 소스(F11)마다 파일 상태, 서비스마다 장부·프로브 캐시.

use super::delta::TokenDelta;
use super::ledger::{Ledger, Vals};
use super::now_secs;
use super::probe::resolve_plan;
use super::roots::{dedup, excluded, expand, glob_under, roots_from, Vars};
use super::spec::{Compiled, ServiceSpec};
use super::sqlite::{self, DbStamp, SqlErr};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(super) const TOKEN_FIELDS: [&str; 4] = ["input", "cache_read", "cache_write", "output"];
const PRIME_WINDOW_SECS: f64 = 2.0 * 24.0 * 3600.0;
/// 서비스 하나의 키 장부 상한(스펙 F2).
const LEDGER_CAP: usize = 500_000;
/// 같은 DB를 이보다 자주 쿼리하지 않는다(스펙 5절).
const SQLITE_GAP_SECS: f64 = 2.0;
/// 커서 없는 쿼리가 이보다 많은 행을 내면 `doctor` 경고.
const SQLITE_BIG_SCAN: usize = 10_000;

pub struct ServiceReader {
    pub spec: ServiceSpec,
    /// 서비스 수준 자리(프로브 키, `live_chars`). 소스 자리는 `Source::x`.
    pub(super) x: Compiled,
    pub(super) sources: Vec<Source>,
    pub(super) plan: String,
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
    offset: HashMap<String, u64>,
    mtime: HashMap<String, f64>,
    pub(super) ctx: HashMap<String, HashMap<String, String>>,
    /// cumulative 기준값: 파일 → 스트림 키 해시(키가 없으면 None) → 값.
    pub(super) base: HashMap<String, HashMap<Option<u64>, Vals>>,
    /// 파일마다 지난 `rebase_on` 값의 해시.
    pub(super) roll: HashMap<String, u64>,
    /// 이번 파일 읽기에서 `rebase_on`이 바뀌었다: 기준값만 잡고 내지 않는다.
    pub(super) rolling: bool,
    pub(super) blind: HashSet<String>,
    /// 파일마다 처음 파싱된 레코드 시각(match와 상관없이, 스펙 4.5). 저장은 2.9.
    pub(super) first: HashMap<String, f64>,
    /// `format: sqlite`의 DB마다 도장·커서.
    pub(super) db: HashMap<String, DbState>,
}

impl Source {
    fn new(spec: ServiceSpec) -> Self {
        // 로더가 이미 검증했다. 검증 없이 만든 스펙(adapter check)의 틀린 식은 아무것도 읽지 않는다.
        let x = Compiled::new(&spec).unwrap_or_default();
        Self { spec, x, ..Default::default() }
    }

    /// 루트 틀을 펴고(B4: 못 펴면 버림) 정규화한 경로로 합친 뒤 패턴으로 찾는다(F10).
    /// ponytail: `roots_from` 레지스트리(1 MB 상한)를 훑을 때마다 다시 읽는다. 폴 비용이 보이면 mtime 캐시(2.11).
    fn files(&self) -> Vec<PathBuf> {
        let vars = Vars { root: None, ctx: &|_| None };
        let fixed = self.spec.roots.iter().filter_map(|r| expand(r, &vars).ok().flatten()).flatten();
        let mut roots: Vec<(PathBuf, Vec<String>)> =
            dedup(fixed.collect()).into_iter().map(|r| (r, self.spec.patterns.clone())).collect();
        for rf in &self.x.roots_from {
            roots.extend(roots_from(rf, &vars).0);
        }
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (root, patterns) in roots {
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
    pub fn new(spec: ServiceSpec) -> Self {
        let x = Compiled::new(&spec).unwrap_or_default();
        let plan = resolve_plan(&spec, x.plan_key.as_ref());
        // 소스가 없으면 서비스 자체가 소스 하나다(F11)
        let views = if spec.sources.is_empty() {
            vec![spec.clone()]
        } else {
            spec.sources.clone()
        };
        Self {
            spec,
            x,
            sources: views.into_iter().map(Source::new).collect(),
            plan,
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
                        all.extend(me.read_in(src, &path, true));
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

    pub fn prime(&mut self) {
        let cutoff = now_secs() - PRIME_WINDOW_SECS;
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| {
                for path in src.files() {
                    let Ok(stat) = fs::metadata(&path) else {
                        continue;
                    };
                    // SQLite는 커서 없이 끝까지 읽어 키만 배우고 커서를 최댓값으로 둔다(스펙 5절 처음 읽기)
                    if src.spec.format == "sqlite" || mtime_of(&stat) >= cutoff {
                        let _ = me.read_in(src, &path, false);
                    } else {
                        let key = path_key(&path);
                        src.offset.insert(key.clone(), stat.len());
                        src.mtime.insert(key.clone(), mtime_of(&stat));
                        src.blind.insert(key);
                    }
                }
            });
        }
    }

    pub fn poll(&mut self) -> Vec<TokenDelta> {
        let mut all = Vec::new();
        for i in 0..self.sources.len() {
            self.with_source(i, |me, src| {
                for path in src.files() {
                    if !src.unchanged(&path) {
                        all.extend(me.read_in(src, &path, true));
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
                    out.extend(me.read_in(src, path, emit));
                }
            });
        }
        out
    }

    fn read_in(&mut self, src: &mut Source, path: &Path, emit: bool) -> Vec<TokenDelta> {
        let mut out = Vec::new();
        src.rolling = false;
        if src.spec.format == "json" {
            self.read_json(src, path, &mut out, emit);
        } else if src.spec.format == "sqlite" {
            self.read_sqlite(src, path, &mut out, emit);
        } else {
            self.read_jsonl(src, path, &mut out, emit);
        }
        out
    }

    fn read_json(&mut self, src: &mut Source, path: &Path, out: &mut Vec<TokenDelta>, emit: bool) {
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
        self.handle(src, &obj, path, out, emit);
        src.mtime.insert(path_key(path), mtime_of(&stat));
    }

    /// 폴 사이에 연결·트랜잭션을 들고 있지 않는다: 열고, 문장 하나를 끝까지 돌리고, 닫는다.
    fn read_sqlite(&mut self, src: &mut Source, path: &Path, out: &mut Vec<TokenDelta>, emit: bool) {
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
                    self.handle(src, row, path, out, emit);
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

    fn read_jsonl(&mut self, src: &mut Source, path: &Path, out: &mut Vec<TokenDelta>, emit: bool) {
        let Ok(stat) = fs::metadata(path) else { return };
        let key = path_key(path);
        let size = stat.len();
        let mut offset = src.offset.get(&key).copied().unwrap_or(0);
        if size < offset {
            offset = 0;
        }
        if size == offset {
            src.mtime.insert(key, mtime_of(&stat));
            return;
        }
        let Ok(mut file) = fs::File::open(path) else {
            return;
        };
        if file.seek(SeekFrom::Start(offset)).is_err() {
            return;
        }
        let mut buf = Vec::with_capacity((size - offset).min(1024 * 1024) as usize);
        if file.read_to_end(&mut buf).is_err() {
            return;
        }
        let Some(end) = buf.iter().rposition(|b| *b == b'\n') else {
            return;
        };
        let mut pos = offset;
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
                self.handle(src, &obj, path, out, emit);
            }
        }
        src.offset.insert(key.clone(), pos);
        src.mtime.insert(key, mtime_of(&stat));
    }
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
