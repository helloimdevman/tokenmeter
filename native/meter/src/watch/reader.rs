//! 파일 찾기, 폴, JSON·JSONL 읽기. 소스(F11)마다 파일 상태, 서비스마다 장부·프로브 캐시.

use super::delta::TokenDelta;
use super::ledger::{Ledger, Vals};
use super::now_secs;
use super::probe::resolve_plan;
use super::roots::expand_home;
use super::spec::{Compiled, ServiceSpec};
use glob::glob;
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
}

impl Source {
    fn new(spec: ServiceSpec) -> Self {
        // 로더가 이미 검증했다. 검증 없이 만든 스펙(adapter check)의 틀린 식은 아무것도 읽지 않는다.
        let x = Compiled::new(&spec).unwrap_or_default();
        Self { spec, x, ..Default::default() }
    }

    fn files(&self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for root in &self.spec.roots {
            let root = expand_home(root);
            if !root.exists() {
                continue;
            }
            for pattern in &self.spec.patterns {
                let pat = root.join(pattern).to_string_lossy().into_owned();
                if let Ok(paths) = glob(&pat) {
                    for p in paths.flatten() {
                        if p.is_file() {
                            out.push(p);
                        }
                    }
                }
            }
        }
        out
    }

    /// 지난 읽기 뒤로 바뀌지 않았다.
    fn unchanged(&self, path: &Path) -> bool {
        let key = path_key(path);
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
        }
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
                    if mtime_of(&stat) >= cutoff {
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
