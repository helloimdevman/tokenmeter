//! JSON/JSONL 로그에서 토큰 델타를 뽑는다. Python ServiceReader 의 핵심 규약만 옮긴다.

use glob::glob;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};
use tokenmeter_hook::{data_dir, live_path};

const TOKEN_FIELDS: [&str; 4] = ["input", "cache_read", "cache_write", "output"];
const PRIME_WINDOW_SECS: f64 = 2.0 * 24.0 * 3600.0;
const SEEN_CAP: usize = 200_000;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default = "default_patterns")]
    pub patterns: Vec<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub match_fields: HashMap<String, MatchWant>,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub input_includes_cache: bool,
    #[serde(default)]
    pub fields: HashMap<String, Option<String>>,
    #[serde(default)]
    pub context: HashMap<String, String>,
    #[serde(default)]
    pub ctx_tokens: Option<String>,
    #[serde(default)]
    pub ctx_window: Option<String>,
    #[serde(default)]
    pub subagent: Option<String>,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub plan_probe: serde_yaml::Value,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub endpoint_probe: serde_yaml::Value,
    #[serde(default)]
    pub live_chars: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<String>,
    #[serde(default)]
    pub install: InstallSpec,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InstallSpec {
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub events: Vec<String>,
}

fn default_patterns() -> Vec<String> {
    vec!["**/*.jsonl".into()]
}
fn default_format() -> String {
    "jsonl".into()
}
fn default_mode() -> String {
    "delta".into()
}

#[derive(Clone, Debug)]
pub enum MatchWant {
    One(String),
    Many(Vec<String>),
}

impl<'de> Deserialize<'de> for MatchWant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        if let Some(items) = value.as_sequence() {
            Ok(Self::Many(items.iter().map(yaml_scalar).collect()))
        } else {
            Ok(Self::One(yaml_scalar(&value)))
        }
    }
}

fn yaml_scalar(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::Null => "null".into(),
        serde_yaml::Value::Bool(value) => value.to_string(),
        serde_yaml::Value::Number(value) => value.to_string(),
        serde_yaml::Value::String(value) => value.clone(),
        value => serde_yaml::to_string(value)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct TokenDelta {
    pub input_tokens: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub output_tokens: i64,
    pub model: String,
    pub service: String,
    pub project: String,
    pub session: String,
    pub vendor: String,
    pub plan: String,
    pub endpoint: String,
    pub cwd: String,
    pub effort: String,
    pub ctx_tokens: i64,
    pub ctx_window: i64,
    pub subagent: bool,
    pub duration_ms: i64,
}

impl TokenDelta {
    pub fn total(&self) -> i64 {
        self.input_tokens + self.cache_read + self.cache_write + self.output_tokens
    }
}

type Vector = [i64; 4];

pub struct ServiceReader {
    pub spec: ServiceSpec,
    plan: String,
    endpoint: HashMap<String, String>,
    offset: HashMap<String, u64>,
    mtime: HashMap<String, f64>,
    lines: HashMap<String, u64>,
    ctx: HashMap<String, HashMap<String, String>>,
    seen: HashMap<String, HashSet<String>>,
    seen_keys: HashSet<String>,
    base: HashMap<String, HashMap<String, Vector>>,
    blind: HashSet<String>,
    live_out: HashMap<String, i64>,
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

    fn handle(
        &mut self,
        obj: &Value,
        path: &Path,
        line_no: u64,
        out: &mut Vec<TokenDelta>,
        emit: bool,
    ) {
        let key = path_key(path);
        let ctx = self.ctx.entry(key.clone()).or_default();
        for (name, dot) in &self.spec.context {
            if let Some(v) = dig_string(obj, dot) {
                if !v.is_empty() {
                    ctx.insert(name.clone(), v);
                }
            }
        }
        if !self.matches(obj) {
            return;
        }
        let vector: Vector = std::array::from_fn(|i| {
            let field = TOKEN_FIELDS[i];
            let path = self.spec.fields.get(field).and_then(|p| p.as_deref());
            num(dig(obj, path.unwrap_or("")))
        });
        let diff: Vector = if self.spec.mode == "cumulative" {
            let record_key = self
                .spec
                .key
                .as_ref()
                .and_then(|k| dig_string(obj, k))
                .unwrap_or_else(|| key.clone());
            let bases = self.base.entry(key.clone()).or_default();
            let previous = bases.get(&record_key).copied();
            bases.insert(record_key, vector);
            match previous {
                None if self.blind.remove(&key) => return,
                None => vector,
                Some(prev) => {
                    let d = [
                        vector[0] - prev[0],
                        vector[1] - prev[1],
                        vector[2] - prev[2],
                        vector[3] - prev[3],
                    ];
                    if d.iter().any(|v| *v < 0) {
                        return;
                    }
                    d
                }
            }
        } else {
            if let Some(k) = &self.spec.key {
                if let Some(raw) = dig_string(obj, k) {
                    if !raw.is_empty() && !self.seen_keys.insert(raw) {
                        return;
                    }
                    if self.seen_keys.len() > SEEN_CAP {
                        let remove: Vec<String> =
                            self.seen_keys.iter().take(SEEN_CAP / 2).cloned().collect();
                        for key in remove {
                            self.seen_keys.remove(&key);
                        }
                    }
                } else {
                    let seen = self.seen.entry(key.clone()).or_default();
                    let mark = format!("{key}:{line_no}");
                    if !seen.insert(mark) {
                        return;
                    }
                }
            } else {
                let seen = self.seen.entry(key.clone()).or_default();
                let mark = format!("{key}:{line_no}");
                if !seen.insert(mark) {
                    return;
                }
            }
            vector
        };
        let mut input_tokens = diff[0];
        let cache_read = diff[1];
        let cache_write = diff[2];
        let mut output_tokens = diff[3];
        if self.spec.input_includes_cache {
            input_tokens = (input_tokens - cache_read).max(0);
        }
        let ctx_map = self.ctx.get(&key).cloned().unwrap_or_default();
        let pick_ctx = |name: &str, fallback: &str| {
            self.spec
                .context
                .get(name)
                .and_then(|dot| dig_string(obj, dot))
                .filter(|s| !s.is_empty())
                .or_else(|| ctx_map.get(name).cloned())
                .unwrap_or_else(|| fallback.to_string())
        };
        let model = pick_ctx("model", &self.spec.default_model);
        let cwd = pick_ctx("cwd", "");
        let vendor = {
            let v = pick_ctx("vendor", "");
            if v.is_empty() {
                if self.spec.vendor.is_empty() {
                    crate::pricing::vendor_of(&model)
                } else {
                    self.spec.vendor.clone()
                }
            } else {
                v
            }
        };
        let session = pick_ctx("session", "");
        let effort = pick_ctx("effort", "");
        let endpoint = self.endpoint_for(&session, &vendor);
        output_tokens = self.adjust_live_output(obj, &session, output_tokens);
        let duration_ms = self
            .spec
            .duration_ms
            .as_ref()
            .map(|p| num(dig(obj, p)))
            .unwrap_or(0);
        let subagent = self
            .spec
            .subagent
            .as_ref()
            .map(|p| dig(obj, p).map(is_truthy).unwrap_or(false))
            .unwrap_or(false);
        let (ctx_now, ctx_win) = if subagent {
            (0, 0)
        } else if let Some(p) = &self.spec.ctx_tokens {
            let current = num(dig(obj, p));
            let window = self
                .spec
                .ctx_window
                .as_ref()
                .map(|w| num(dig(obj, w)))
                .filter(|n| *n > 0)
                .unwrap_or_else(|| crate::pricing::context_window(&model, current));
            (current, window)
        } else {
            let current = vector[0] + vector[1] + vector[2];
            (current, crate::pricing::context_window(&model, current))
        };
        let mut delta = TokenDelta {
            input_tokens,
            cache_read,
            cache_write,
            output_tokens,
            model,
            service: self.spec.name.clone(),
            project: tokenmeter_hook::project_key(&cwd),
            session,
            vendor,
            plan: self.plan.clone(),
            endpoint,
            cwd,
            effort,
            ctx_tokens: ctx_now,
            ctx_window: ctx_win,
            subagent,
            duration_ms,
        };
        if delta.total() <= 0 {
            return;
        }
        if emit {
            out.push(std::mem::take(&mut delta));
        }
    }

    fn matches(&self, obj: &Value) -> bool {
        for (dot, want) in &self.spec.match_fields {
            let got = dig_string(obj, dot).unwrap_or_else(|| "null".into());
            match want {
                MatchWant::Many(items) => {
                    if !items.iter().any(|w| w == &got) {
                        return false;
                    }
                }
                MatchWant::One(w) => {
                    if &got != w {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn endpoint_for(&mut self, session: &str, vendor: &str) -> String {
        let key = format!("{session}|{vendor}");
        if let Some(value) = self.endpoint.get(&key) {
            return value.clone();
        }
        let env = if session.is_empty() {
            None
        } else {
            fs::read_to_string(live_path(&self.spec.name, session))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|value| value.get("routing_env").and_then(Value::as_object).cloned())
                .map(|values| {
                    values
                        .into_iter()
                        .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
                        .collect::<HashMap<_, _>>()
                })
        };
        let value = resolve_endpoint(&self.spec, env.as_ref(), vendor, &self.plan);
        if self.endpoint.len() > 1000 {
            self.endpoint.clear();
        }
        self.endpoint.insert(key, value.clone());
        value
    }

    fn adjust_live_output(&mut self, obj: &Value, session: &str, output_tokens: i64) -> i64 {
        let Some(path) = &self.spec.live_chars else {
            return output_tokens;
        };
        let extra = dig_string(obj, path)
            .map(|t| {
                if t.is_empty() {
                    0
                } else {
                    t.chars().count().div_ceil(4).max(1) as i64
                }
            })
            .unwrap_or(0);
        let prompt = dig_string(obj, "params._meta.promptId")
            .or_else(|| dig_string(obj, "params.update.prompt_id"))
            .unwrap_or_default();
        let turn = if prompt.is_empty() {
            session.to_string()
        } else {
            format!("{session}|{prompt}")
        };
        if output_tokens > 0 {
            let prev = self.live_out.remove(&turn).unwrap_or(0);
            return (output_tokens - prev).max(0);
        }
        if extra <= 0 {
            return output_tokens;
        }
        *self.live_out.entry(turn).or_insert(0) += extra;
        if self.live_out.len() > 10_000 {
            let remove: Vec<String> = self.live_out.keys().take(5000).cloned().collect();
            for key in remove {
                self.live_out.remove(&key);
            }
        }
        extra
    }
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn mtime_of(stat: &fs::Metadata) -> f64 {
    stat.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

pub(crate) fn expand_home(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("$TOKENMETER_HOME") {
        let rest = rest.trim_start_matches(['/', '\\']);
        return if rest.is_empty() {
            data_dir()
        } else {
            data_dir().join(rest)
        };
    }
    let expanded = expand_vars(raw);
    if let Some(rest) = expanded.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(expanded)
}

fn expand_vars(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let (start, end, next) = if chars.get(i + 1) == Some(&'{') {
            let Some(close) = chars[i + 2..].iter().position(|c| *c == '}') else {
                out.push('$');
                i += 1;
                continue;
            };
            (i + 2, i + 2 + close, i + 3 + close)
        } else {
            let mut end = i + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            (i + 1, end, end)
        };
        if start == end {
            out.push('$');
        } else {
            let name: String = chars[start..end].iter().collect();
            out.push_str(&std::env::var(name).unwrap_or_default());
        }
        i = next;
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

pub fn dig<'a>(obj: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let mut cur = obj;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn dig_string(obj: &Value, path: &str) -> Option<String> {
    match dig(obj, path)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

fn num(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0).max(0.0) as i64,
        Some(Value::String(s)) => s.parse::<f64>().ok().unwrap_or(0.0).max(0.0) as i64,
        _ => 0,
    }
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => s != "false" && s != "0" && !s.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

#[derive(Deserialize)]
struct YamlFile {
    #[serde(default)]
    services: HashMap<String, YamlService>,
}

#[derive(Deserialize, Default)]
struct YamlService {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    patterns: Option<Vec<String>>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default, rename = "match")]
    match_fields: HashMap<String, MatchWant>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    input_includes_cache: Option<bool>,
    #[serde(default)]
    fields: HashMap<String, Option<String>>,
    #[serde(default)]
    context: HashMap<String, Option<String>>,
    #[serde(default)]
    ctx_tokens: Option<String>,
    #[serde(default)]
    ctx_window: Option<String>,
    #[serde(default)]
    subagent: Option<String>,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    vendor: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    plan_probe: serde_yaml::Value,
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    endpoint_probe: serde_yaml::Value,
    #[serde(default)]
    live_chars: Option<String>,
    #[serde(default)]
    duration_ms: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    install: Option<InstallSpec>,
}

const DEFAULT_YAML: &str = include_str!("../services.yaml");

pub fn default_specs() -> Vec<ServiceSpec> {
    specs_from_yaml(DEFAULT_YAML)
}

/// 패키지에 들어 있는 서비스인지. 사용자가 추가한 서비스 이름은 업로드하지 않는다.
pub fn is_builtin_service(name: &str) -> bool {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES
        .get_or_init(|| default_specs().into_iter().map(|s| s.name).collect())
        .iter()
        .any(|n| n == name)
}

#[derive(Clone, Debug)]
pub struct RuntimeSettings {
    pub enabled: bool,
    pub overlay_auto: bool,
    pub poll_seconds: f64,
    pub idle_exit_minutes: f64,
    pub live_ttl_hours: f64,
    pub session_history: usize,
    pub full_scale: f64,
    pub attention_notify: bool,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            overlay_auto: true,
            poll_seconds: 2.0,
            idle_exit_minutes: 30.0,
            live_ttl_hours: 6.0,
            session_history: 500,
            full_scale: 3000.0,
            attention_notify: true,
        }
    }
}

pub struct RuntimeConfig {
    pub specs: Vec<ServiceSpec>,
    pub settings: RuntimeSettings,
}

pub fn load_merged_yaml() -> serde_yaml::Value {
    let mut raw = serde_yaml::from_str::<serde_yaml::Value>(DEFAULT_YAML)
        .unwrap_or(serde_yaml::Value::Mapping(Default::default()));
    let user_path = config_dir().join("services.yaml");
    if let Ok(text) = fs::read_to_string(user_path) {
        if let Ok(user) = serde_yaml::from_str::<serde_yaml::Value>(&text) {
            deep_merge_yaml(&mut raw, user);
        }
    }
    raw
}

pub fn setting_str(path: &[&str]) -> String {
    yaml_at(&load_merged_yaml(), path)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

pub fn setting_f64(path: &[&str], default: f64) -> f64 {
    yaml_f64(&load_merged_yaml(), path).unwrap_or(default)
}

pub fn setting_map(path: &[&str]) -> HashMap<String, String> {
    let Some(serde_yaml::Value::Mapping(map)) = yaml_at(&load_merged_yaml(), path).cloned() else {
        return HashMap::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| {
            let key = match k {
                serde_yaml::Value::String(s) => s,
                other => yaml_scalar(&other),
            };
            let val = match v {
                serde_yaml::Value::String(s) => s,
                other => yaml_scalar(&other),
            };
            Some((key, val))
        })
        .collect()
}

pub fn setting_list(path: &[&str]) -> Vec<String> {
    yaml_at(&load_merged_yaml(), path)
        .and_then(|v| v.as_sequence())
        .map(|rows| {
            rows.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn load_all_specs() -> Vec<ServiceSpec> {
    let text = serde_yaml::to_string(&load_merged_yaml()).unwrap_or_default();
    specs_from_yaml_opts(&text, true)
}

pub fn load_runtime_config() -> RuntimeConfig {
    let raw = load_merged_yaml();
    let text = serde_yaml::to_string(&raw).unwrap_or_default();
    let mut specs = specs_from_yaml(&text);
    let mut settings = RuntimeSettings {
        poll_seconds: yaml_f64(&raw, &["settings", "poll_seconds"])
            .unwrap_or(2.0)
            .max(0.2),
        idle_exit_minutes: yaml_f64(&raw, &["settings", "idle_exit_minutes"])
            .unwrap_or(30.0)
            .max(0.0),
        live_ttl_hours: yaml_f64(&raw, &["settings", "live_ttl_hours"])
            .unwrap_or(6.0)
            .max(0.0),
        session_history: yaml_f64(&raw, &["settings", "session_history"])
            .unwrap_or(500.0)
            .max(20.0) as usize,
        full_scale: yaml_f64(&raw, &["settings", "overlay", "full_scale"])
            .unwrap_or(3000.0)
            .max(1.0),
        attention_notify: yaml_bool(&raw, &["settings", "attention_notify"]).unwrap_or_else(|| {
            yaml_f64(&raw, &["settings", "idle_notify_seconds"]).unwrap_or(0.0) != 0.0
        }),
        ..Default::default()
    };

    if let Ok(text) = fs::read_to_string(data_dir().join("toggle.json")) {
        if let Ok(toggle) = serde_json::from_str::<Value>(&text) {
            settings.enabled = toggle.get("enabled").and_then(Value::as_bool) != Some(false);
            settings.overlay_auto = toggle.get("overlay").and_then(Value::as_bool) != Some(false);
            if let Some(book) = toggle.get("services").and_then(Value::as_object) {
                specs.retain(|spec| book.get(&spec.name).and_then(Value::as_bool) != Some(false));
            }
        }
    }
    RuntimeConfig { specs, settings }
}

pub fn specs_from_yaml(text: &str) -> Vec<ServiceSpec> {
    specs_from_yaml_opts(text, false)
}

pub fn specs_from_yaml_opts(text: &str, include_disabled: bool) -> Vec<ServiceSpec> {
    let parsed: YamlFile = serde_yaml::from_str(text).unwrap_or(YamlFile {
        services: HashMap::new(),
    });
    let mut out = Vec::new();
    for (name, raw) in parsed.services {
        let enabled = raw.enabled != Some(false);
        if !enabled && !include_disabled {
            continue;
        }
        out.push(ServiceSpec {
            name: name.clone(),
            label: raw.label.unwrap_or(name),
            enabled,
            roots: raw.roots,
            patterns: raw.patterns.unwrap_or_else(default_patterns),
            format: raw.format.unwrap_or_else(default_format),
            match_fields: raw.match_fields,
            mode: raw.mode.unwrap_or_else(default_mode),
            key: raw.key,
            input_includes_cache: raw.input_includes_cache.unwrap_or(false),
            fields: raw.fields,
            // `cwd: null` 같은 빈 자리는 경로가 아니다 (serde_yaml 은 String 에 "null" 을 넣는다)
            context: raw.context.into_iter().filter_map(|(k, v)| Some((k, v?))).collect(),
            ctx_tokens: raw.ctx_tokens,
            ctx_window: raw.ctx_window,
            subagent: raw.subagent,
            default_model: raw.default_model.unwrap_or_default(),
            vendor: raw.vendor.unwrap_or_default(),
            plan: raw.plan.unwrap_or_default(),
            plan_probe: raw.plan_probe,
            endpoint: raw.endpoint.unwrap_or_default(),
            endpoint_probe: raw.endpoint_probe,
            live_chars: raw.live_chars,
            duration_ms: raw.duration_ms,
            install: raw.install.unwrap_or_default(),
        });
    }
    out
}

fn config_dir() -> PathBuf {
    if let Ok(path) = std::env::var("XDG_CONFIG_HOME") {
        if !path.trim().is_empty() {
            return PathBuf::from(path).join("tokenmeter");
        }
    }
    home_dir().unwrap_or_default().join(".config/tokenmeter")
}

fn deep_merge_yaml(base: &mut serde_yaml::Value, over: serde_yaml::Value) {
    match (base, over) {
        (serde_yaml::Value::Mapping(base), serde_yaml::Value::Mapping(over)) => {
            for (key, value) in over {
                if let Some(existing) = base.get_mut(&key) {
                    deep_merge_yaml(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, over) => *base = over,
    }
}

fn yaml_at<'a>(value: &'a serde_yaml::Value, path: &[&str]) -> Option<&'a serde_yaml::Value> {
    let mut current = value;
    for part in path {
        current = current
            .as_mapping()?
            .get(serde_yaml::Value::String((*part).into()))?;
    }
    Some(current)
}

fn yaml_f64(value: &serde_yaml::Value, path: &[&str]) -> Option<f64> {
    let value = yaml_at(value, path)?;
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn yaml_bool(value: &serde_yaml::Value, path: &[&str]) -> Option<bool> {
    let value = yaml_at(value, path)?;
    value
        .as_bool()
        .or_else(|| match value.as_str()?.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        })
}

fn yaml_string(value: &serde_yaml::Value, key: &str) -> String {
    yaml_at(value, &[key])
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn yaml_strings(value: &serde_yaml::Value, key: &str) -> Vec<String> {
    yaml_at(value, &[key])
        .and_then(|v| v.as_sequence())
        .map(|rows| {
            rows.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn resolve_plan(spec: &ServiceSpec) -> String {
    if !spec.plan.is_empty() {
        return spec.plan.clone();
    }
    let probe = &spec.plan_probe;
    let env = yaml_strings(probe, "env");
    if !env.is_empty() {
        let set = env
            .iter()
            .any(|name| std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false));
        let value = yaml_string(probe, if set { "if_set" } else { "else" });
        return if value.is_empty() {
            "unknown".into()
        } else {
            value
        };
    }
    let path = yaml_string(probe, "path");
    let key = yaml_string(probe, "key");
    let default = {
        let value = yaml_string(probe, "default");
        if value.is_empty() {
            "unknown".into()
        } else {
            value
        }
    };
    if path.is_empty() || key.is_empty() {
        return default;
    }
    let Some(value) = probe_file(&expand_home(&path), &key) else {
        return default;
    };
    let text = json_text(&value);
    yaml_at(probe, &["map"])
        .and_then(|m| m.as_mapping())
        .and_then(|m| m.get(serde_yaml::Value::String(text.clone())))
        .and_then(|v| v.as_str())
        .unwrap_or(&default)
        .to_string()
}

fn resolve_endpoint(
    spec: &ServiceSpec,
    session_env: Option<&HashMap<String, String>>,
    vendor: &str,
    plan: &str,
) -> String {
    let probe = &spec.endpoint_probe;
    let env_value = |name: &str| {
        session_env
            .and_then(|env| env.get(name).cloned())
            .or_else(|| std::env::var(name).ok())
            .unwrap_or_default()
    };
    if let Some(flags) = yaml_at(probe, &["flags"]).and_then(|v| v.as_mapping()) {
        for (name, label) in flags {
            let Some(name) = name.as_str() else { continue };
            let value = env_value(name).to_lowercase();
            if !value.is_empty() && !matches!(value.as_str(), "0" | "false" | "no") {
                return label.as_str().unwrap_or_default().to_string();
            }
        }
    }
    for name in yaml_strings(probe, "env") {
        let value = env_value(&name);
        if !value.trim().is_empty() {
            return value;
        }
    }
    let path = yaml_string(probe, "path");
    let key = yaml_string(probe, "key").replace("{vendor}", vendor);
    if !path.is_empty() && !key.is_empty() {
        if let Some(value) = probe_file(&expand_home(&path), &key) {
            let text = json_text(&value);
            if !text.is_empty() {
                return text;
            }
        }
    }
    if let Some(default) = yaml_at(probe, &["default"]) {
        if let Some(book) = default.as_mapping() {
            return book
                .get(serde_yaml::Value::String(plan.to_string()))
                .or_else(|| book.get(serde_yaml::Value::String("unknown".into())))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
        }
        if let Some(value) = default.as_str() {
            return value.to_string();
        }
    }
    spec.endpoint.clone()
}

fn probe_file(path: &Path, key: &str) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    if path.extension().and_then(|value| value.to_str()) == Some("toml") {
        return probe_toml(&text, key);
    }
    let value = serde_json::from_str::<Value>(&text).ok()?;
    dig(&value, key).cloned()
}

fn probe_toml(text: &str, key: &str) -> Option<Value> {
    let (section, field) = key.rsplit_once('.')?;
    let mut current = "";
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') && line.ends_with(']') {
            current = line.trim_matches(&['[', ']'][..]).trim();
            continue;
        }
        if current != section {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() != field {
            continue;
        }
        let value = value.trim();
        if (value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\''))
        {
            return Some(Value::String(value[1..value.len() - 1].to_string()));
        }
        if let Ok(value) = value.parse::<bool>() {
            return Some(Value::Bool(value));
        }
        if let Ok(value) = value.parse::<i64>() {
            return Some(Value::Number(value.into()));
        }
        return value
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number);
    }
    None
}

fn json_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(v) => v.to_string(),
        _ => String::new(),
    }
}

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    #[test]
    fn config_merge_and_toml_probe_match_python() {
        let mut base: serde_yaml::Value = serde_yaml::from_str(
            "settings: {poll_seconds: 2}\nservices: {codex: {enabled: true, roots: [a]}}",
        )
        .unwrap();
        let over = serde_yaml::from_str("services: {codex: {roots: [b]}}").unwrap();
        deep_merge_yaml(&mut base, over);
        assert_eq!(yaml_f64(&base, &["settings", "poll_seconds"]), Some(2.0));
        assert_eq!(yaml_at(&base, &["services", "codex", "roots", "0"]), None);
        let roots = yaml_at(&base, &["services", "codex", "roots"])
            .and_then(|value| value.as_sequence())
            .unwrap();
        assert_eq!(roots[0].as_str(), Some("b"));
        let parsed =
            specs_from_yaml("services: {custom: {match: {enabled: true, code: 7}, roots: []}}");
        assert!(
            matches!(parsed[0].match_fields["enabled"], MatchWant::One(ref value) if value == "true")
        );
        assert!(
            matches!(parsed[0].match_fields["code"], MatchWant::One(ref value) if value == "7")
        );

        let path =
            std::env::temp_dir().join(format!("tokenmeter-config-{}.toml", std::process::id()));
        fs::write(
            &path,
            "[model_providers.openai]\nbase_url = \"https://example.test/v1\"\n",
        )
        .unwrap();
        assert_eq!(
            probe_file(&path, "model_providers.openai.base_url")
                .and_then(|value| value.as_str().map(str::to_string)),
            Some("https://example.test/v1".into())
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn tokenmeter_home_root_uses_data_dir() {
        let _g = crate::TEST_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!("tokenmeter-home-{}", std::process::id()));
        std::env::set_var("TOKENMETER_HOME", &home);
        assert_eq!(expand_home("$TOKENMETER_HOME/cursor"), home.join("cursor"));
    }

    /// 패키지 services.yaml 의 진짜 스펙에 roots 만 갈아끼운다.
    fn spec_at(name: &str, root: &Path) -> ServiceSpec {
        let mut spec = default_specs()
            .into_iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("services.yaml 에 {name} 가 없다"));
        spec.roots = vec![root.to_string_lossy().into_owned()];
        spec
    }

    fn vec4(d: &TokenDelta) -> (i64, i64, i64, i64) {
        (d.input_tokens, d.cache_read, d.cache_write, d.output_tokens)
    }

    fn append(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
        f.write_all(text.as_bytes()).unwrap();
    }

    fn lines(records: &[Value]) -> String {
        records.iter().map(|r| format!("{r}\n")).collect()
    }

    /// json 포맷은 mtime 으로 변경을 본다 — 같은 초에 두 번 써도 놓치지 않게 못 박는다.
    fn write_json(path: &Path, value: &Value, tick: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value.to_string()).unwrap();
        let when = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick);
        fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    fn claude_record(uuid: &str) -> Value {
        json!({
            "type": "assistant", "uuid": uuid, "cwd": "/Users/dev/projects/tokenmeter",
            "sessionId": "sess-claude-1",
            "message": {"model": "claude-opus-5", "usage": {
                "input_tokens": 2, "cache_creation_input_tokens": 2161,
                "cache_read_input_tokens": 60955, "output_tokens": 813
            }}
        })
    }

    fn codex_record(inp: i64, cached: i64, write: i64, out: i64, last: i64, window: i64) -> Value {
        json!({"type": "event_msg", "payload": {"type": "token_count", "info": {
            "total_token_usage": {
                "input_tokens": inp, "cached_input_tokens": cached,
                "cache_write_input_tokens": write, "output_tokens": out,
                "reasoning_output_tokens": 129
            },
            "last_token_usage": {"total_tokens": last},
            "model_context_window": window
        }}})
    }

    fn grok_turn(event: &str, inp: i64, cached: i64, write: i64, out: i64, duration_ms: i64) -> Value {
        let mut usage = json!({
            "inputTokens": inp, "cachedReadTokens": cached, "cacheCreationTokens": write,
            "outputTokens": out, "reasoningTokens": 7308, "totalTokens": inp + out
        });
        if duration_ms > 0 {
            usage["apiDurationMs"] = json!(duration_ms);
        }
        json!({"method": "_x.ai/session/update", "params": {
            "sessionId": "sess-grok-1", "_meta": {"eventId": event},
            "update": {"sessionUpdate": "turn_completed", "prompt_id": "prompt-1", "usage": usage}
        }})
    }

    fn grok_chunk(event: &str, text: &str, thought: bool, total: i64) -> Value {
        json!({"method": "session/update", "params": {
            "sessionId": "sess-grok-1",
            "_meta": {"eventId": event, "promptId": "prompt-1", "totalTokens": total},
            "update": {
                "sessionUpdate": if thought { "agent_thought_chunk" } else { "agent_message_chunk" },
                "content": {"type": "text", "text": text}
            }
        }})
    }

    #[test]
    fn committed_fixtures_regress_every_enabled_service() {
        let _home = crate::test_home("fixtures");
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures")
            .canonicalize()
            .unwrap();
        let want = [
            ("claude-code", (2, 60955, 2161, 813), "claude-opus-5", "sess-claude-1", "projects/tokenmeter", "anthropic"),
            ("codex", (16023, 34304, 0, 384), "gpt-5.6-sol", "sess-codex-1", "projects/tokenmeter", "openai"),
            ("opencode", (3265, 25344, 0, 88), "nemotron-3-ultra-free", "ses_1", "Users/dev", "opencode"),
            ("grok", (122167, 1052672, 0, 12855), "grok-4.6-build", "sess-grok-1", "", "xai"),
            ("cursor", (15, 80, 5, 7), "cursor-grok-4.6", "s1", "work/token-pet", "cursor"),
        ];
        let mut enabled: Vec<String> = default_specs().into_iter().map(|s| s.name).collect();
        enabled.sort();
        let mut covered: Vec<String> = want.iter().map(|w| w.0.to_string()).collect();
        covered.sort();
        assert_eq!(enabled, covered, "켜진 서비스마다 fixture 가 있어야 한다");
        for (name, vector, model, session, project, vendor) in want {
            let deltas = ServiceReader::new(spec_at(name, &root.join(name))).poll();
            assert_eq!(deltas.len(), 1, "{name}");
            let d = &deltas[0];
            assert_eq!(vec4(d), vector, "{name}");
            assert_eq!(
                (d.service.as_str(), d.model.as_str(), d.session.as_str(), d.project.as_str(), d.vendor.as_str()),
                (name, model, session, project, vendor),
                "{name}"
            );
        }
    }

    #[test]
    fn enabled_services_declare_token_fields() {
        for spec in default_specs() {
            let has = |f: &str| spec.fields.get(f).cloned().flatten().is_some();
            assert!(has("output") || has("input"), "{}", spec.name);
        }
    }

    #[test]
    fn dig_walks_objects_and_array_indexes() {
        let obj = json!({"a": {"b": [{"c": 1}, {"c": 2}]}, "n": null, "zero": 0});
        assert_eq!(dig(&obj, "a.b.1.c"), Some(&json!(2)));
        assert_eq!(dig(&obj, "a.b.0.c"), Some(&json!(1)));
        assert_eq!(dig(&obj, "zero"), Some(&json!(0)));
        for missing in ["a.b.9.c", "a.b.x.c", "a.nope", "n.deeper", ""] {
            assert_eq!(dig(&obj, missing), None, "{missing}");
        }
    }

    #[test]
    fn claude_delta_dedups_uuid_filters_type_and_skips_broken_lines() {
        let (_g, tmp) = crate::test_home("claude");
        let root = tmp.join("projects");
        let path = root.join("slug/sess-1.jsonl");
        append(&path, &lines(&[claude_record("u-1")]));
        let mut reader = ServiceReader::new(spec_at("claude-code", &root));
        let got = reader.poll();
        assert_eq!(got.len(), 1);
        assert_eq!(vec4(&got[0]), (2, 60955, 2161, 813));
        assert!(matches!(got[0].plan.as_str(), "subscription" | "api"), "{}", got[0].plan);

        append(&path, &lines(&[claude_record("u-1")]));
        assert!(reader.poll().is_empty(), "같은 uuid 를 두 번 먹으면 안 된다");
        let mut user = claude_record("u-2");
        user["type"] = json!("user");
        append(&path, &lines(&[user]));
        assert!(reader.poll().is_empty(), "match(type=assistant) 밖은 무시한다");
        append(&path, "{\"type\":\"assistant\",\"uuid\"\n");
        append(&path, &lines(&[claude_record("u-3")]));
        let got = reader.poll();
        assert_eq!(got.len(), 1, "깨진 줄은 건너뛰고 다음 줄은 먹는다");
        assert_eq!(vec4(&got[0]), (2, 60955, 2161, 813));
    }

    #[test]
    fn prime_skips_history_and_forked_copies_are_not_recounted() {
        let (_g, tmp) = crate::test_home("prime");
        let root = tmp.join("projects");
        let old: Vec<Value> = (0..5).map(|i| claude_record(&format!("u-{i}"))).collect();
        append(&root.join("slug/a.jsonl"), &lines(&old));
        let mut reader = ServiceReader::new(spec_at("claude-code", &root));
        reader.prime();
        assert!(reader.poll().is_empty(), "기동 시 과거 로그를 먹으면 안 된다");

        let mut fresh = claude_record("u-new");
        fresh["message"]["usage"] = json!({
            "input_tokens": 7, "cache_creation_input_tokens": 0,
            "cache_read_input_tokens": 0, "output_tokens": 11
        });
        let mut fork = old.clone();
        fork.push(fresh);
        append(&root.join("slug/b.jsonl"), &lines(&fork));
        let got = reader.poll();
        assert_eq!(got.len(), 1, "fork 로 복사된 레코드를 다시 먹으면 이중계상이다");
        assert_eq!(vec4(&got[0]), (7, 0, 0, 11));
    }

    #[test]
    fn codex_cumulative_takes_increments_and_rebaselines() {
        let (_g, tmp) = crate::test_home("codex");
        let root = tmp.join("sessions");
        let path = root.join("2026/08/10/rollout-1.jsonl");
        let cwd = "/Users/dev/projects/tokenmeter";
        append(&path, &lines(&[
            json!({"type": "session_meta", "payload": {"cwd": cwd, "session_id": "sess-codex-1", "model_provider": "openai"}}),
            json!({"type": "turn_context", "payload": {"cwd": cwd, "model": "gpt-5.6-sol"}}),
            codex_record(50327, 34304, 0, 384, 0, 0),
        ]));
        let mut reader = ServiceReader::new(spec_at("codex", &root));
        let got = reader.poll();
        assert_eq!(vec4(&got[0]), (16023, 34304, 0, 384), "input 은 캐시를 포함, reasoning 은 더하지 않는다");
        assert_eq!(
            (got[0].model.as_str(), got[0].project.as_str(), got[0].session.as_str(), got[0].vendor.as_str()),
            ("gpt-5.6-sol", "projects/tokenmeter", "sess-codex-1", "openai")
        );
        append(&path, &lines(&[codex_record(60327, 40304, 100, 584, 0, 0)]));
        assert_eq!(vec4(&reader.poll()[0]), (4000, 6000, 100, 200));
        append(&path, &lines(&[codex_record(60327, 40304, 100, 584, 0, 0)]));
        assert!(reader.poll().is_empty(), "같은 누적치는 증가분 0");
        append(&path, &lines(&[codex_record(10, 0, 0, 5, 0, 0)]));
        assert!(reader.poll().is_empty(), "누적치가 줄면 baseline 만 갱신한다");
        append(&path, &lines(&[codex_record(110, 0, 0, 15, 0, 0)]));
        assert_eq!(vec4(&reader.poll()[0]), (100, 0, 0, 10));

        let other = root.join("2026/08/11/rollout-2.jsonl");
        append(&other, &lines(&[codex_record(50000, 0, 0, 1000, 0, 0)]));
        let mut primed = ServiceReader::new(spec_at("codex", &root));
        primed.prime();
        assert!(primed.poll().is_empty());
        append(&other, &lines(&[codex_record(50500, 0, 0, 1200, 0, 0)]));
        let got = primed.poll();
        assert_eq!(got.len(), 1);
        assert_eq!(vec4(&got[0]), (500, 0, 0, 200), "prime 은 현재 누적치를 baseline 으로 잡는다");
    }

    #[test]
    fn opencode_message_file_counts_once_when_completed() {
        let (_g, tmp) = crate::test_home("opencode");
        let root = tmp.join("message");
        let path = root.join("ses_1/msg_1.json");
        let mut rec = json!({
            "id": "msg_1", "role": "assistant", "sessionID": "ses_1",
            "modelID": "nemotron-3-ultra-free", "providerID": "opencode",
            "path": {"cwd": "/Users/dev"}, "cost": 0,
            "tokens": {"total": 0, "input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}
        });
        write_json(&path, &rec, 1);
        let mut reader = ServiceReader::new(spec_at("opencode", &root));
        assert!(reader.poll().is_empty(), "생성 시점(토큰 0)에는 먹지 않는다");
        rec["tokens"] = json!({"total": 28697, "input": 3265, "output": 88, "reasoning": 68,
                               "cache": {"read": 25344, "write": 0}});
        write_json(&path, &rec, 2);
        let got = reader.poll();
        assert_eq!(got.len(), 1);
        assert_eq!(vec4(&got[0]), (3265, 25344, 0, 88));
        assert_eq!(
            (got[0].model.as_str(), got[0].project.as_str(), got[0].session.as_str(),
             got[0].vendor.as_str(), got[0].plan.as_str()),
            ("nemotron-3-ultra-free", "Users/dev", "ses_1", "opencode", "api"),
            "벤더는 모델명이 아니라 게이트웨이(providerID)다"
        );
        write_json(&path, &rec, 3);
        assert!(reader.poll().is_empty(), "같은 내용이 다시 쓰여도 증가분 0");
    }

    #[test]
    fn grok_turns_dedup_and_streaming_chunks_estimate_output() {
        let (_g, tmp) = crate::test_home("grok");
        let root = tmp.join("sessions");
        let path = root.join("%2FUsers%2Fdev/sess-grok-1/updates.jsonl");
        append(&path, &lines(&[
            json!({"method": "_x.ai/session/update", "params": {"sessionId": "sess-grok-1",
                   "_meta": {"totalTokens": 100986}, "update": {"sessionUpdate": "agent_message_chunk"}}}),
            grok_turn("e-1", 1174839, 1052672, 0, 12855, 0),
        ]));
        let mut reader = ServiceReader::new(spec_at("grok", &root));
        let got = reader.poll();
        assert_eq!(got.len(), 1, "본문 없는 스트리밍 줄은 먹지 않는다");
        assert_eq!(vec4(&got[0]), (122167, 1052672, 0, 12855));
        assert_eq!((got[0].model.as_str(), got[0].vendor.as_str(), got[0].session.as_str()),
                   ("grok-4.6-build", "xai", "sess-grok-1"));
        assert_eq!(got[0].ctx_tokens, 0, "turn_completed 입력 합계로 ctx% 를 지어내면 안 된다");
        append(&path, &lines(&[grok_turn("e-1", 1174839, 1052672, 0, 12855, 0)]));
        assert!(reader.poll().is_empty(), "같은 eventId 는 한 번만");
        append(&path, &lines(&[grok_turn("e-2", 20000, 15000, 100, 300, 0)]));
        assert_eq!(vec4(&reader.poll()[0]), (5000, 15000, 100, 300));

        let stream = tmp.join("stream");
        let path = stream.join("sess/updates.jsonl");
        append(&path, &lines(&[grok_chunk("c-1", &"abcd".repeat(3), true, 1000)]));
        let mut reader = ServiceReader::new(spec_at("grok", &stream));
        let got = reader.poll();
        assert_eq!((got[0].output_tokens, got[0].ctx_tokens, got[0].duration_ms), (3, 1000, 0));
        append(&path, &lines(&[grok_chunk("c-2", "xyz", false, 1100)]));
        let got = reader.poll();
        assert_eq!((got[0].output_tokens, got[0].ctx_tokens), (1, 1100));
        append(&path, &lines(&[grok_turn("e-1", 100, 0, 0, 40, 8000)]));
        let got = reader.poll();
        assert_eq!(got[0].output_tokens, 36, "턴이 끝나면 공식 출력과의 차(40 - 3 - 1)만 더한다");
        assert_eq!(got[0].duration_ms, 8000);
    }

    #[test]
    fn context_tokens_follow_each_service_spec() {
        let (_g, tmp) = crate::test_home("ctx");
        let root = tmp.join("projects");
        let mut reader = ServiceReader::new(spec_at("claude-code", &root));
        append(&root.join("slug/s.jsonl"), &lines(&[claude_record("u-1")]));
        let got = reader.poll();
        assert_eq!(got[0].ctx_tokens, 2 + 60955 + 2161, "출력은 컨텍스트가 아니다");
        assert_eq!(got[0].ctx_window, 1_000_000, "가격표의 window 가 분모다");

        let mut big = claude_record("u-big");
        big["message"]["usage"] = json!({"input_tokens": 2, "cache_read_input_tokens": 430_000,
                                         "cache_creation_input_tokens": 5_000, "output_tokens": 100});
        append(&root.join("slug/big.jsonl"), &lines(&[big.clone()]));
        let got = reader.poll();
        assert!(got[0].ctx_tokens > 200_000 && got[0].ctx_window == 1_000_000);

        big["uuid"] = json!("u-sub");
        big["isSidechain"] = json!(true);
        append(&root.join("slug/sub.jsonl"), &lines(&[big]));
        let got = reader.poll();
        assert!(got[0].total() > 0 && got[0].subagent, "서브에이전트 토큰은 합산한다");
        assert_eq!(got[0].ctx_tokens, 0, "남의 컨텍스트가 세션 ctx% 를 덮으면 안 된다");

        let croot = tmp.join("sessions");
        let mut creader = ServiceReader::new(spec_at("codex", &croot));
        append(&croot.join("r.jsonl"), &lines(&[
            json!({"type": "turn_context", "payload": {"cwd": "/a/tokenmeter", "model": "gpt-5.6-sol"}}),
            codex_record(4_657_501, 4_343_296, 0, 20_577, 123_165, 353_400),
        ]));
        let got = creader.poll();
        assert_eq!(got[0].ctx_tokens, 123_165, "세션 누적이 컨텍스트로 잡히면 늘 100% 다");
        assert_eq!(got[0].ctx_window, 353_400, "로그가 알려주는 창이 가격표보다 우선한다");
        append(&croot.join("r.jsonl"), &lines(&[codex_record(4_700_000, 4_343_296, 0, 20_800, 12_000, 353_400)]));
        assert_eq!(creader.poll()[0].ctx_tokens, 12_000, "압축되면 그대로 내려간다");
    }

    #[test]
    fn plan_and_endpoint_probes_follow_env_files_and_live_routing() {
        let (_g, tmp) = crate::test_home("probe");
        for var in ["TP_FAKE_KEY", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX"] {
            std::env::remove_var(var);
        }
        let yaml = |text: String| serde_yaml::from_str::<serde_yaml::Value>(&text).unwrap();
        let env_probe = yaml("{env: [TP_FAKE_KEY], if_set: api, else: subscription}".into());
        let explicit = ServiceSpec { plan: "subscription".into(), plan_probe: env_probe.clone(), ..Default::default() };
        assert_eq!(resolve_plan(&explicit), "subscription", "명시값이 항상 이긴다");
        let by_env = ServiceSpec { plan_probe: env_probe, ..Default::default() };
        assert_eq!(resolve_plan(&by_env), "subscription");
        std::env::set_var("TP_FAKE_KEY", "sk-1");
        assert_eq!(resolve_plan(&by_env), "api");
        std::env::remove_var("TP_FAKE_KEY");

        let auth = tmp.join("auth.json");
        let by_file = ServiceSpec {
            plan_probe: yaml(format!(
                "{{path: {}, key: auth_mode, map: {{chatgpt: subscription, apikey: api}}, default: unknown}}",
                json!(auth.display().to_string())
            )),
            ..Default::default()
        };
        fs::write(&auth, r#"{"auth_mode": "chatgpt"}"#).unwrap();
        assert_eq!(resolve_plan(&by_file), "subscription");
        fs::write(&auth, r#"{"auth_mode": "apikey"}"#).unwrap();
        assert_eq!(resolve_plan(&by_file), "api");
        fs::write(&auth, "깨진 파일").unwrap();
        assert_eq!(resolve_plan(&by_file), "unknown", "프로브가 실패해도 죽으면 안 된다");
        fs::remove_file(&auth).unwrap();
        assert_eq!(resolve_plan(&by_file), "unknown");
        assert_eq!(resolve_plan(&ServiceSpec::default()), "unknown");

        for (model, vendor) in [("claude-opus-5", "anthropic"), ("gpt-5.6-sol", "openai"),
                                ("nemotron-3-ultra-free", "nvidia"), ("무슨-모델", "unknown"), ("", "unknown")] {
            assert_eq!(crate::pricing::vendor_of(model), vendor, "{model}");
        }

        let claude = spec_at("claude-code", &tmp);
        let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
        };
        assert_eq!(resolve_endpoint(&claude, Some(&env(&[])), "anthropic", "api"), "https://api.anthropic.com");
        assert_eq!(
            resolve_endpoint(&claude, Some(&env(&[("ANTHROPIC_BASE_URL", "https://llm.mycorp.com/v1")])), "anthropic", "api"),
            "https://llm.mycorp.com/v1"
        );
        assert_eq!(resolve_endpoint(&claude, Some(&env(&[("CLAUDE_CODE_USE_BEDROCK", "1")])), "anthropic", "api"), "bedrock");

        fs::create_dir_all(tokenmeter_hook::live_dir()).unwrap();
        fs::write(live_path("claude-code", "s-bedrock"), r#"{"routing_env": {"CLAUDE_CODE_USE_BEDROCK": "1"}}"#).unwrap();
        let mut reader = ServiceReader::new(claude);
        assert_eq!(reader.endpoint_for("s-bedrock", "anthropic"), "bedrock", "훅이 찍은 세션 환경을 읽는다");
        assert_eq!(reader.endpoint_for("s-없음", "anthropic"), "https://api.anthropic.com");
    }

    #[test]
    fn toggle_file_switches_measurement_services_and_overlay() {
        let (_g, _tmp) = crate::test_home("toggle");
        let toggle = data_dir().join("toggle.json");
        let names = || -> Vec<String> { load_runtime_config().specs.into_iter().map(|s| s.name).collect() };
        let settings = || load_runtime_config().settings;
        assert!(settings().enabled && settings().overlay_auto, "파일이 없으면 켜진 것으로 본다");
        fs::write(&toggle, r#"{"enabled": false}"#).unwrap();
        assert!(!settings().enabled, "전체를 끄면 데몬이 뜨지 않는다");
        fs::write(&toggle, r#"{"services": {"codex": false}}"#).unwrap();
        assert!(settings().enabled);
        assert!(!names().contains(&"codex".to_string()) && names().contains(&"claude-code".to_string()));
        fs::write(&toggle, r#"{"overlay": false}"#).unwrap();
        assert!(!settings().overlay_auto && settings().enabled, "창을 끈 것이 측정을 끄면 안 된다");
        fs::write(&toggle, "{망가짐").unwrap();
        assert!(settings().enabled, "깨진 파일 하나 때문에 측정이 멈추면 안 된다");
    }
}
