//! 요금제·엔드포인트 프로브: 환경 변수, 설정 파일(JSON·TOML), 세션 라우팅.

use super::expr::dig;
use super::roots::expand_home;
use super::spec::{yaml_at, yaml_string, yaml_strings, ServiceSpec};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub(super) fn resolve_plan(spec: &ServiceSpec) -> String {
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

pub(super) fn resolve_endpoint(
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

pub(super) fn probe_file(path: &Path, key: &str) -> Option<Value> {
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
