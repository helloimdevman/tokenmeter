//! 서비스 스펙: YAML 로딩, 사용자 덮어쓰기 병합, 설정 읽기.

use super::roots::home_dir;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;
use tokenmeter_hook::data_dir;

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

const DEFAULT_YAML: &str = include_str!("../../services.yaml");

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

pub(super) fn deep_merge_yaml(base: &mut serde_yaml::Value, over: serde_yaml::Value) {
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

pub(super) fn yaml_at<'a>(value: &'a serde_yaml::Value, path: &[&str]) -> Option<&'a serde_yaml::Value> {
    let mut current = value;
    for part in path {
        current = current
            .as_mapping()?
            .get(serde_yaml::Value::String((*part).into()))?;
    }
    Some(current)
}

pub(super) fn yaml_f64(value: &serde_yaml::Value, path: &[&str]) -> Option<f64> {
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

pub(super) fn yaml_string(value: &serde_yaml::Value, key: &str) -> String {
    yaml_at(value, &[key])
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

pub(super) fn yaml_strings(value: &serde_yaml::Value, key: &str) -> Vec<String> {
    yaml_at(value, &[key])
        .and_then(|v| v.as_sequence())
        .map(|rows| {
            rows.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
