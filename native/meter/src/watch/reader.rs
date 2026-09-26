//! 파일 찾기, 폴, JSON·JSONL 읽기.

use super::delta::{TokenDelta, Vector};
use super::now_secs;
use super::probe::resolve_plan;
use super::roots::expand_home;
use super::spec::ServiceSpec;
use glob::glob;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub(super) const TOKEN_FIELDS: [&str; 4] = ["input", "cache_read", "cache_write", "output"];
const PRIME_WINDOW_SECS: f64 = 2.0 * 24.0 * 3600.0;
pub(super) const SEEN_CAP: usize = 200_000;

pub struct ServiceReader {
    pub spec: ServiceSpec,
    pub(super) plan: String,
    pub(super) endpoint: HashMap<String, String>,
    offset: HashMap<String, u64>,
    mtime: HashMap<String, f64>,
    lines: HashMap<String, u64>,
    pub(super) ctx: HashMap<String, HashMap<String, String>>,
    pub(super) seen: HashMap<String, HashSet<String>>,
    pub(super) seen_keys: HashSet<String>,
    pub(super) base: HashMap<String, HashMap<String, Vector>>,
    pub(super) blind: HashSet<String>,
    pub(super) live_out: HashMap<String, i64>,
}

impl ServiceReader {
    pub fn new(spec: ServiceSpec) -> Self {
        let plan = resolve_plan(&spec);
        Self {
            spec,
            plan,
            endpoint: HashMap::new(),
            offset: HashMap::new(),
            mtime: HashMap::new(),
            lines: HashMap::new(),
            ctx: HashMap::new(),
            seen: HashMap::new(),
            seen_keys: HashSet::new(),
            base: HashMap::new(),
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
        self.handle(&obj, path, 0, out, emit);
        self.mtime.insert(path_key(path), mtime_of(&stat));
    }

    fn read_jsonl(&mut self, path: &Path, out: &mut Vec<TokenDelta>, emit: bool) {
        let Ok(stat) = fs::metadata(path) else { return };
        let key = path_key(path);
        let size = stat.len();
        let mut offset = self.offset.get(&key).copied().unwrap_or(0);
        if size < offset {
            offset = 0;
            self.lines.remove(&key);
            if self.spec.key.as_deref().unwrap_or("").is_empty() {
                self.seen.remove(&key);
            }
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
        let mut line_no = self.lines.get(&key).copied().unwrap_or(0);
        for chunk in buf[..end].split(|b| *b == b'\n') {
            pos += chunk.len() as u64 + 1;
            line_no += 1;
            if chunk.is_empty() {
                continue;
            }
            let text = String::from_utf8_lossy(chunk);
            let text = text.trim();
            if text.is_empty() {
                continue;
            }
            if let Ok(obj) = serde_json::from_str::<Value>(text) {
                self.handle(&obj, path, line_no, out, emit);
            }
        }
        self.offset.insert(key.clone(), pos);
        self.lines.insert(key.clone(), line_no);
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
