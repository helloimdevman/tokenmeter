//! 개인 로그를 남기지 않는 서비스 어댑터 초안.

use crate::watch::{dig, specs_from_yaml};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const SECRETISH: &[&str] = &["key", "token", "secret", "password", "credential", "auth", "cookie"];
const ALIASES: &[(&str, &[&str])] = &[
    ("input", &["input_tokens", "input", "prompt_tokens"]),
    (
        "cache_read",
        &["cache_read_input_tokens", "cached_input_tokens", "cache_read_tokens"],
    ),
    (
        "cache_write",
        &[
            "cache_creation_input_tokens",
            "cache_write_input_tokens",
            "cache_write_tokens",
        ],
    ),
    (
        "output",
        &["output_tokens", "output", "completion_tokens", "generated_tokens"],
    ),
    ("cwd", &["cwd", "working_directory", "workspace"]),
    ("model", &["model", "model_id", "modelid", "model_name"]),
    ("session", &["session_id", "sessionid", "session", "conversation_id"]),
];

fn safe_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

pub fn redact_fixture(value: &Value, key: &str) -> Result<Value, String> {
    let secret = !key.is_empty()
        && SECRETISH
            .iter()
            .any(|n| key.to_ascii_lowercase().contains(n));
    match value {
        Value::Object(map) => {
            let mut clean = serde_json::Map::new();
            for (raw, item) in map {
                if !safe_key(raw) {
                    return Err("객체 키는 안전한 스키마 식별자여야 합니다".into());
                }
                clean.insert(raw.clone(), redact_fixture(item, raw)?);
            }
            Ok(if secret { json!("<redacted>") } else { Value::Object(clean) })
        }
        Value::Array(items) => {
            let clean: Result<Vec<_>, _> = items.iter().map(|i| redact_fixture(i, "")).collect();
            Ok(if secret { json!("<redacted>") } else { Value::Array(clean?) })
        }
        _ if secret => Ok(json!("<redacted>")),
        Value::Bool(_) => Ok(json!(false)),
        Value::String(_) => Ok(json!("")),
        Value::Number(_) => Ok(json!(0)),
        Value::Null => Ok(Value::Null),
    }
}

fn collect_paths(value: &Value, prefix: &str, out: &mut std::collections::HashMap<String, String>) {
    match value {
        Value::Object(map) => {
            for (key, item) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                out.entry(key.to_ascii_lowercase()).or_insert(path.clone());
                collect_paths(item, &path, out);
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let path = if prefix.is_empty() {
                    i.to_string()
                } else {
                    format!("{prefix}.{i}")
                };
                collect_paths(item, &path, out);
            }
        }
        _ => {}
    }
}

fn find_path(paths: &std::collections::HashMap<String, String>, name: &str) -> Option<String> {
    ALIASES
        .iter()
        .find(|(n, _)| *n == name)
        .and_then(|(_, aliases)| aliases.iter().find_map(|a| paths.get(*a).cloned()))
}

fn latest_log(path: &Path) -> Result<PathBuf, String> {
    if path.is_file() {
        return Ok(path.to_path_buf());
    }
    if !path.is_dir() {
        return Err(format!("로그 경로를 찾을 수 없습니다: {}", path.display()));
    }
    let mut found = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                walk(&p, out);
            } else if matches!(
                p.extension().and_then(|s| s.to_str()),
                Some("json") | Some("jsonl")
            ) {
                out.push(p);
            }
        }
    }
    walk(path, &mut found);
    found
        .into_iter()
        .max_by_key(|p| p.metadata().and_then(|m| m.modified()).ok())
        .ok_or_else(|| "JSON 또는 JSONL 로그를 찾지 못했습니다".into())
}

fn read_record(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("로그를 읽을 수 없습니다: {e}"))?;
    if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
        for line in text.lines().rev().take(100) {
            if let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) {
                return Ok(Value::Object(obj));
            }
        }
        return Err("유효한 JSON 객체를 찾지 못했습니다".into());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(obj)) => Ok(Value::Object(obj)),
        Ok(_) => Err("JSON 로그는 객체여야 합니다".into()),
        Err(e) => Err(format!("로그를 읽을 수 없습니다: {e}")),
    }
}

fn service_yaml(name: &str, record: &Value, log_path: &Path, requested: &Path) -> String {
    let mut paths = std::collections::HashMap::new();
    collect_paths(record, "", &mut paths);
    let fmt = if log_path.extension().and_then(|s| s.to_str()) == Some("json") {
        "json"
    } else {
        "jsonl"
    };
    let root = if requested.is_dir() {
        requested
    } else {
        requested.parent().unwrap_or(requested)
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let home = home.canonicalize().unwrap_or(home);
    let root_text = root
        .strip_prefix(&home)
        .map(|p| format!("~/{}", p.display()))
        .unwrap_or_else(|_| root.display().to_string());
    let field = |n: &str| find_path(&paths, n).unwrap_or_default();
    format!(
        "# mode, key, match는 로그 의미를 확인한 뒤 선택하세요.\n\
services:\n  {name}:\n    enabled: true\n    label: {name}\n    roots: [{root_text:?}]\n    \
patterns: [\"**/*.{fmt}\"]\n    format: {fmt}\n    mode: choose-delta-or-cumulative\n    key: null\n    \
match: {{}}\n    fields:\n      input: {input}\n      cache_read: {cache_read}\n      \
cache_write: {cache_write}\n      output: {output}\n    context:\n      cwd: {cwd}\n      \
model: {model}\n      session: {session}\n    default_model: default\n    \
install:\n      target: none\n",
        input = yaml_opt(&field("input")),
        cache_read = yaml_opt(&field("cache_read")),
        cache_write = yaml_opt(&field("cache_write")),
        output = yaml_opt(&field("output")),
        cwd = yaml_opt(&field("cwd")),
        model = yaml_opt(&field("model")),
        session = yaml_opt(&field("session")),
    )
}

fn yaml_opt(value: &str) -> String {
    if value.is_empty() {
        "null".into()
    } else {
        format!("{value:?}")
    }
}

pub fn init_adapter(name: &str, log_path: &Path, output: &Path) -> Result<String, String> {
    if output.exists() {
        let empty = output.is_dir() && output.read_dir().map(|mut d| d.next().is_none()).unwrap_or(false);
        if !empty {
            return Err(format!("출력 디렉터리가 비어 있지 않습니다: {}", output.display()));
        }
    }
    let selected = latest_log(log_path)?;
    let record = read_record(&selected)?;
    let fixture = redact_fixture(&record, "")?;
    let fixture_text = format!("{}\n", serde_json::to_string_pretty(&fixture).unwrap_or_default());
    let service_text = service_yaml(name, &record, &selected, log_path);
    fs::create_dir_all(output).map_err(|e| format!("어댑터를 쓸 수 없습니다: {e}"))?;
    let mut created = Vec::new();
    for (filename, text) in [("fixture.json", fixture_text), ("service.yaml", service_text)] {
        let path = output.join(filename);
        if let Err(e) = fs::write(&path, text) {
            for old in &created {
                let _ = fs::remove_file(old);
            }
            return Err(format!("어댑터를 쓸 수 없습니다: {e}"));
        }
        created.push(path);
    }
    Ok(format!("어댑터 초안을 만들었습니다: {}", output.display()))
}

pub fn check_adapter(path: &Path) -> (bool, Vec<String>) {
    let yaml = match fs::read_to_string(path.join("service.yaml")) {
        Ok(t) => t,
        Err(e) => return (false, vec![format!("어댑터를 읽을 수 없습니다: {e}")]),
    };
    let fixture = match fs::read_to_string(path.join("fixture.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
    {
        Some(v) => v,
        None => return (false, vec!["어댑터를 읽을 수 없습니다: fixture.json".into()]),
    };
    let specs = specs_from_yaml(&yaml);
    if specs.len() != 1 {
        return (
            false,
            vec!["service.yaml에는 services 아래 서비스가 정확히 하나 있어야 합니다".into()],
        );
    }
    let spec = &specs[0];
    let mut errors = Vec::new();
    if spec.mode != "delta" && spec.mode != "cumulative" {
        errors.push("mode: delta 또는 cumulative 중 하나를 선택하세요".into());
    }
    for (field, dot) in &spec.fields {
        if let Some(dot) = dot.as_str() {
            if !dot.is_empty() && dig(&fixture, dot).is_none() {
                errors.push(format!("fields.{field}: {dot} 경로가 fixture.json에 없습니다"));
            }
        }
    }
    for (field, dot) in &spec.context {
        let dot = dot.as_str().unwrap_or_default();
        if !dot.is_empty() && dig(&fixture, dot).is_none() {
            errors.push(format!("context.{field}: {dot} 경로가 fixture.json에 없습니다"));
        }
    }
    if errors.is_empty() {
        let connected = ["input", "cache_read", "cache_write", "output"]
            .iter()
            .filter_map(|k| Some(format!("{k}={}", spec.fields.get(*k)?.as_str()?)))
            .collect::<Vec<_>>()
            .join(", ");
        (
            true,
            vec![format!(
                "{}: 구조 검증 통과 (연결된 토큰 필드: {})",
                spec.name,
                if connected.is_empty() { "없음" } else { &connected }
            )],
        )
    } else {
        (false, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str, tick: u64) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick);
        fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    fn service(dir: &Path, name: &str) -> serde_yaml::Value {
        let text = fs::read_to_string(dir.join("service.yaml")).unwrap();
        serde_yaml::from_str::<serde_yaml::Value>(&text).unwrap()["services"][name].clone()
    }

    #[test]
    fn redact_blanks_values_and_hides_secretish_keys() {
        let source = json!({"api_key": "sk-secret", "usage": {"input_tokens": 42},
                            "model": "private-model", "ok": true, "items": ["secret"]});
        assert_eq!(
            redact_fixture(&source, "").unwrap(),
            json!({"api_key": "<redacted>", "usage": {"input_tokens": "<redacted>"},
                   "model": "", "ok": false, "items": [""]})
        );
        assert_eq!(
            redact_fixture(&json!({"items": [{"auth_token": "nested-secret", "count": 7, "none": null}], "ratio": 1.5}), "").unwrap(),
            json!({"items": [{"auth_token": "<redacted>", "count": 0, "none": null}], "ratio": 0})
        );
    }

    #[test]
    fn init_writes_two_redacted_files_once() {
        let (_g, tmp) = crate::test_home("adapter-init");
        let log = tmp.join("agent.json");
        write(&log, r#"{"api_key": "sk-secret", "usage": {"input_tokens": 42}, "model": "private-model"}"#, 1);
        let out = tmp.join("sample-adapter");
        assert!(init_adapter("sample", &log, &out).is_ok());
        let before = fs::read_to_string(out.join("fixture.json")).unwrap();
        assert!(out.join("service.yaml").exists());
        assert!(init_adapter("sample", &log, &out).is_err(), "기존 초안을 덮어쓰면 안 된다");
        assert_eq!(fs::read_to_string(out.join("fixture.json")).unwrap(), before);
        for secret in ["sk-secret", "private-model", "42"] {
            assert!(!before.contains(secret), "{secret}");
        }
    }

    #[test]
    fn init_picks_newest_log_and_home_relative_root() {
        let (_g, tmp) = crate::test_home("adapter-root");
        let logs = tmp.join("logs");
        write(&logs.join("old.json"), r#"{"usage": {"input_tokens": 1}}"#, 1);
        write(&logs.join("nested/new.jsonl"), "{\"fresh\": {\"input\": 2}}\n", 2);
        let out = tmp.join("dir-adapter");
        assert!(init_adapter("sample", &logs, &out).is_ok());
        let fixture: Value = serde_json::from_str(&fs::read_to_string(out.join("fixture.json")).unwrap()).unwrap();
        assert_eq!(fixture, json!({"fresh": {"input": 0}}), "가장 최근 로그를 골라야 한다");
        let svc = service(&out, "sample");
        assert_eq!(svc["roots"][0].as_str(), Some(logs.canonicalize().unwrap().to_str().unwrap()));
        assert_eq!(svc["patterns"][0].as_str(), Some("**/*.jsonl"));

        let file_log = tmp.join("file-logs/private-name.json");
        write(&file_log, r#"{"usage": {"output_tokens": 1}}"#, 3);
        let out = tmp.join("file-adapter");
        assert!(init_adapter("file", &file_log, &out).is_ok());
        let svc = service(&out, "file");
        let parent = file_log.parent().unwrap().canonicalize().unwrap();
        assert_eq!(svc["roots"][0].as_str(), Some(parent.to_str().unwrap()), "root 는 파일이 아니라 부모 디렉터리");
        assert_eq!(svc["patterns"][0].as_str(), Some("**/*.json"));
        assert!(!fs::read_to_string(out.join("service.yaml")).unwrap().contains("private-name"));

        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let home_log = home.join("agent/logs/private.json");
        write(&home_log, r#"{"usage": {"input": 1}}"#, 4);
        let out = tmp.join("home-adapter");
        assert!(init_adapter("home", &home_log, &out).is_ok());
        let text = fs::read_to_string(out.join("service.yaml")).unwrap();
        assert_eq!(service(&out, "home")["roots"][0].as_str(), Some("~/agent/logs"));
        assert!(!text.contains(home.to_str().unwrap()), "홈 경로가 초안에 남으면 안 된다");
    }

    #[test]
    fn init_refuses_unsafe_keys_and_broken_logs_before_writing() {
        let (_g, tmp) = crate::test_home("adapter-refuse");
        for (i, key) in ["secret.txt", "../private/path", "git status"].into_iter().enumerate() {
            let log = tmp.join(format!("unsafe-{i}.json"));
            write(&log, &json!({"safe": {"auth": {key: {"output_tokens": 1}}}}).to_string(), 1);
            let out = tmp.join(format!("unsafe-{i}-adapter"));
            let err = init_adapter("sample", &log, &out).unwrap_err();
            assert!(err.contains("객체 키") && !err.contains(key), "{err}");
            assert!(!out.exists());
        }
        for (name, raw) in [
            ("broken.json", &b"{"[..]),
            ("broken.jsonl", &b"not-json\n{\n"[..]),
            ("bad-utf8.json", &b"{\"safe\":\"\xff\"}"[..]),
            ("bad-utf8.jsonl", &b"{\"safe\":\"\xff\"}\n"[..]),
        ] {
            let log = tmp.join(name);
            fs::write(&log, raw).unwrap();
            let out = tmp.join(format!("{name}-adapter"));
            let err = init_adapter("sample", &log, &out).unwrap_err();
            assert!(!name.starts_with("bad-utf8") || err.starts_with("로그를 읽을 수 없습니다:"), "{err}");
            assert!(!out.exists(), "{name}: 깨진 로그로 출력 디렉터리를 만들면 안 된다");
        }
    }

    #[test]
    fn check_requires_one_service_known_paths_and_a_mode() {
        let (_g, tmp) = crate::test_home("adapter-check");
        let log = tmp.join("agent.jsonl");
        write(&log, "{\"usage\": {\"input_tokens\": 1, \"output_tokens\": 2}, \"model\": \"private-model\", \"session_id\": \"private-session\"}\n", 1);
        let out = tmp.join("sample-adapter");
        assert!(init_adapter("sample", &log, &out).is_ok());
        assert_eq!(check_adapter(&out), (false, vec!["mode: delta 또는 cumulative 중 하나를 선택하세요".to_string()]));
        let yaml = fs::read_to_string(out.join("service.yaml")).unwrap();
        fs::write(out.join("service.yaml"), yaml.replace("mode: choose-delta-or-cumulative", "mode: cumulative")).unwrap();
        assert_eq!(
            check_adapter(&out),
            (true, vec!["sample: 구조 검증 통과 (연결된 토큰 필드: input=usage.input_tokens, output=usage.output_tokens)".to_string()])
        );

        let dir = tmp.join("adapter");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("fixture.json"), "{}").unwrap();
        for services in ["services: {}", "services: {one: {}, two: {}}"] {
            fs::write(dir.join("service.yaml"), services).unwrap();
            assert_eq!(check_adapter(&dir), (false, vec!["service.yaml에는 services 아래 서비스가 정확히 하나 있어야 합니다".to_string()]));
        }
        fs::write(dir.join("service.yaml"), "services: {one: {mode: delta, fields: {input: usage.input_tokens}, context: {}}}").unwrap();
        assert_eq!(check_adapter(&dir), (false, vec!["fields.input: usage.input_tokens 경로가 fixture.json에 없습니다".to_string()]));
    }
}
