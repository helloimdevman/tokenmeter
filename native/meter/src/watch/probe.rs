//! 요금제·엔드포인트 프로브: 환경 변수, 설정 파일(JSON·TOML), 세션 라우팅.

use super::expr::{Env, Pick};
use super::roots::{expand_home, read_outside};
use super::spec::{yaml_at, yaml_string, yaml_strings, ServiceSpec};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tokenmeter_hook::normalize_endpoint;

/// 요금제(F9): `plan` → `plan_probe`(벤더마다, `$ctx.vendor`) → `unknown`.
/// `key`는 `plan_probe.key`를 파싱한 것(`Compiled::plan_key`). 경로 표 힌트(`local`, `sub`)는 4.3이 앞뒤에 끼운다.
pub(super) fn resolve_plan(spec: &ServiceSpec, key: Option<&Pick>, vendor: &str) -> String {
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
    let Some(value) = probe_file(&expand_home(&path), key, vendor) else {
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

/// 엔드포인트(스펙 9절, 레코드의 `context.endpoint`는 부르는 쪽이 먼저 본다):
/// 플래그 → env 이름들(세션 `routing_env` 전부 → 데몬 환경 전부) → 파일 프로브 → `default` → 벤더를 맨 id로.
/// 결과는 `normalize_endpoint`를 거친다(사용자 정보·쿼리를 지운다).
/// `key`는 `endpoint_probe.key`를 파싱한 것(`Compiled::endpoint_key`).
pub(super) fn resolve_endpoint(
    spec: &ServiceSpec,
    key: Option<&Pick>,
    session_env: Option<&HashMap<String, String>>,
    vendor: &str,
    plan: &str,
) -> String {
    let probe = &spec.endpoint_probe;
    let daemon = |name: &str| std::env::var(name).ok();
    let session = |name: &str| session_env.and_then(|env| env.get(name).cloned());
    let layers: [&dyn Fn(&str) -> Option<String>; 2] = [&session, &daemon];
    // 층마다 이름 순서로 처음 값이 있는 것
    let first_set = |names: &[String]| {
        layers.iter().find_map(|get| {
            names
                .iter()
                .filter_map(|name| get(name))
                .find(|v| !v.trim().is_empty())
        })
    };
    let on = |v: &str| {
        let v = v.trim().to_lowercase();
        !v.is_empty() && !matches!(v.as_str(), "0" | "false" | "no")
    };
    // 플래그 값은 id, 또는 `{env: [...], default: id}`(base URL이 있으면 그것)
    let flagged = || {
        let flags = yaml_at(probe, &["flags"])?.as_mapping()?;
        layers.iter().find_map(|get| {
            flags.iter().find_map(|(name, target)| {
                get(name.as_str()?).filter(|v| on(v))?;
                match target.as_str() {
                    Some(id) => Some(id.to_string()),
                    None => first_set(&yaml_strings(target, "env"))
                        .or_else(|| Some(yaml_string(target, "default"))),
                }
            })
        })
    };
    let from_file = || {
        let path = yaml_string(probe, "path");
        if path.is_empty() {
            return None;
        }
        Some(json_text(&probe_file(&expand_home(&path), key?, vendor)?))
    };
    let from_default = || {
        let default = yaml_at(probe, &["default"])?;
        let value = match default.as_mapping() {
            Some(book) => book
                .get(serde_yaml::Value::String(plan.to_string()))
                .or_else(|| book.get(serde_yaml::Value::String("unknown".into())))?,
            None => default,
        };
        value.as_str().map(str::to_string)
    };
    let env = || first_set(&yaml_strings(probe, "env"));
    let steps: [&dyn Fn() -> Option<String>; 4] = [&flagged, &env, &from_file, &from_default];
    let found = steps
        .iter()
        .find_map(|step| step().filter(|v| !v.trim().is_empty()))
        .unwrap_or_else(|| vendor.to_string());
    normalize_endpoint(&found)
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
