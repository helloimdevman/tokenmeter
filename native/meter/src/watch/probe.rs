//! 요금제·엔드포인트 프로브: 환경 변수, 설정 파일(JSON·TOML), 세션 라우팅.

use super::expr::{Env, Pick};
use super::roots::{expand_home, read_outside};
use super::spec::{yaml_at, yaml_string, yaml_strings, ServiceSpec};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// `key`는 `plan_probe.key`를 파싱한 것(`Compiled::plan_key`).
pub(super) fn resolve_plan(spec: &ServiceSpec, key: Option<&Pick>) -> String {
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
    let default = {
        let value = yaml_string(probe, "default");
        if value.is_empty() {
            "unknown".into()
        } else {
            value
        }
    };
    let (false, Some(key)) = (path.is_empty(), key) else {
        return default;
    };
    let Some(value) = probe_file(&expand_home(&path), key, &spec.vendor) else {
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

/// `key`는 `endpoint_probe.key`를 파싱한 것(`Compiled::endpoint_key`).
pub(super) fn resolve_endpoint(
    spec: &ServiceSpec,
    key: Option<&Pick>,
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
    if let (false, Some(key)) = (path.is_empty(), key) {
        if let Some(value) = probe_file(&expand_home(&path), key, vendor) {
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

/// 설정 파일(TOML·JSON) 안의 값 하나. 키는 경로식이고 `$ctx.vendor`가 벤더다.
pub(super) fn probe_file(path: &Path, key: &Pick, vendor: &str) -> Option<Value> {
    let doc = read_outside(path)?;
    let ctx = |name: &str| (name == "vendor").then(|| vendor.to_string());
    key.value(&Env::new(&doc, &ctx))
}

fn json_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(v) => v.to_string(),
        _ => String::new(),
    }
}
