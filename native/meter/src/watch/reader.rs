//! 파일 찾기, 폴, JSON·JSONL 읽기.

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
    pub(super) x: Compiled,
    pub(super) plan: String,
    pub(super) endpoint: HashMap<String, String>,
    offset: HashMap<String, u64>,
    mtime: HashMap<String, f64>,
    pub(super) ctx: HashMap<String, HashMap<String, String>>,
    /// delta 키(없으면 레코드 해시)의 칸별 최댓값. 서비스에 하나(F2).
    pub(super) ledger: Ledger,
    /// cumulative 기준값: 파일 → 스트림 키 해시(키가 없으면 None) → 값.
    pub(super) base: HashMap<String, HashMap<Option<u64>, Vals>>,
    /// 파일마다 지난 `rebase_on` 값의 해시.
    pub(super) roll: HashMap<String, u64>,
    /// 이번 파일 읽기에서 `rebase_on`이 바뀌었다: 기준값만 잡고 내지 않는다.
    pub(super) rolling: bool,
    pub(super) blind: HashSet<String>,
    pub(super) live_out: HashMap<String, i64>,
}

impl ServiceReader {
    pub fn new(spec: ServiceSpec) -> Self {
        // 로더가 이미 검증했다. 검증 없이 만든 스펙(adapter check)의 틀린 식은 아무것도 읽지 않는다.
        let x = Compiled::new(&spec).unwrap_or_default();
        let plan = resolve_plan(&spec, x.plan_key.as_ref());
        Self {
            spec,
            x,
            plan,
            endpoint: HashMap::new(),
            offset: HashMap::new(),
            mtime: HashMap::new(),
            ctx: HashMap::new(),
            ledger: Ledger::new(now_secs, LEDGER_CAP),
            base: HashMap::new(),
            roll: HashMap::new(),
            rolling: false,
            blind: HashSet::new(),
            live_out: HashMap::new(),
        }
    }

    pub fn files(&self) -> Vec<PathBuf> {
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

    pub fn prime(&mut self) {
        let cutoff = now_secs() - PRIME_WINDOW_SECS;
        for path in self.files() {
            let Ok(stat) = fs::metadata(&path) else {
                continue;
            };
            if mtime_of(&stat) >= cutoff {
                let _ = self.read_file(&path, false);
            } else {
                let key = path_key(&path);
                self.offset.insert(key.clone(), stat.len());
                self.mtime.insert(key.clone(), mtime_of(&stat));
                self.blind.insert(key);
            }
        }
    }

    pub fn poll(&mut self) -> Vec<TokenDelta> {
        let mut all = Vec::new();
        for path in self.files() {
            let key = path_key(&path);
            if let Ok(stat) = fs::metadata(&path) {
                if self.mtime.get(&key).copied() == Some(mtime_of(&stat))
                    && (self.spec.format == "json"
                        || self.offset.get(&key).copied().unwrap_or(0) >= stat.len())
                {
                    continue;
                }
            }
            all.extend(self.read_file(&path, true));
        }
        all
    }

    pub fn read_file(&mut self, path: &Path, emit: bool) -> Vec<TokenDelta> {
        let mut out = Vec::new();
        self.rolling = false;
        if self.spec.format == "json" {
            self.read_json(path, &mut out, emit);
        } else {
            self.read_jsonl(path, &mut out, emit);
        }
        out
    }

    fn read_json(&mut self, path: &Path, out: &mut Vec<TokenDelta>, emit: bool) {
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
        self.handle(&obj, path, out, emit);
        self.mtime.insert(path_key(path), mtime_of(&stat));
    }

    fn read_jsonl(&mut self, path: &Path, out: &mut Vec<TokenDelta>, emit: bool) {
        let Ok(stat) = fs::metadata(path) else { return };
        let key = path_key(path);
        let size = stat.len();
        let mut offset = self.offset.get(&key).copied().unwrap_or(0);
        if size < offset {
            offset = 0;
        }
        if size == offset {
            self.mtime.insert(key, mtime_of(&stat));
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
                self.handle(&obj, path, out, emit);
            }
        }
        self.offset.insert(key.clone(), pos);
        self.mtime.insert(key, mtime_of(&stat));
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
