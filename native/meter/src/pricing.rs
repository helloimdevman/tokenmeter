//! Python 원본과 같은 최소 가격/컨텍스트 규칙.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy)]
pub struct Price {
    pub input: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub output: f64,
    pub window: i64,
}

const DEFAULT: Price = Price {
    input: 3.0,
    cache_read: 0.3,
    cache_write: 3.75,
    output: 15.0,
    window: 0,
};

fn key(name: &str) -> String {
    name.trim().to_lowercase().replace(['_', ' '], "-")
}

/// 첫 번째로 맞는 행이 이긴다. (계열 이름, (input, cache_read, cache_write, output, window))
fn family_row(model: &str) -> Option<(&'static str, (f64, f64, f64, f64, i64))> {
    let n = key(model);
    Some(if n.contains("fable-5.1") || n.contains("fable-5-1") {
        ("claude-fable-5.1", (10.0, 0.25, 12.5, 50.0, 1_000_000))
    } else if n.contains("claude-fable-5") || n.contains("fable") {
        ("claude-fable-5", (10.0, 1.0, 12.5, 50.0, 1_000_000))
    } else if n.contains("claude-opus-5") {
        ("claude-opus-5", (5.0, 0.5, 6.25, 25.0, 1_000_000))
    } else if n.contains("claude-opus-4.8") || n.contains("opus") {
        ("claude-opus-4.8", (5.0, 0.5, 6.25, 25.0, 200_000))
    } else if n.contains("claude-sonnet-5") {
        ("claude-sonnet-5", (2.0, 0.2, 2.5, 10.0, 1_000_000))
    } else if n.contains("claude-sonnet-4.6") || n.contains("sonnet") {
        ("claude-sonnet-4.6", (3.0, 0.3, 3.75, 15.0, 200_000))
    } else if n.contains("claude-haiku-4.5") || n.contains("haiku") {
        ("claude-haiku-4.5", (1.0, 0.1, 1.25, 5.0, 200_000))
    } else if n.contains("gpt-5.6-luna") || n.contains("luna") {
        ("gpt-5.6-luna", (0.2, 0.02, 0.2, 1.2, 400_000))
    } else if n.contains("gpt-5.6-terra") || n.contains("terra") {
        ("gpt-5.6-terra", (2.0, 0.2, 2.0, 12.0, 400_000))
    } else if n.contains("gpt-5.6-sol") || n.contains("gpt-5.6") || n.contains("sol") {
        ("gpt-5.6-sol", (5.0, 0.5, 5.0, 30.0, 400_000))
    } else if n.contains("gpt-5.4") {
        ("gpt-5.4", (2.5, 0.25, 2.5, 15.0, 400_000))
    } else if n.contains("deepseek") && n.contains("flash") {
        ("deepseek-flash", (0.14, 0.014, 0.14, 0.28, 128_000))
    } else if n.contains("deepseek") {
        ("deepseek", (0.435, 0.0435, 0.435, 0.87, 128_000))
    } else if n.contains("grok-4.6-build") {
        ("grok-4.6-build", (2.0, 0.5, 2.0, 6.0, 500_000))
    } else if n.contains("grok") {
        ("grok", (2.0, 0.5, 2.0, 6.0, 256_000))
    } else {
        return None;
    })
}

fn built_in(model: &str) -> Price {
    let Some((_, row)) = family_row(model) else {
        return DEFAULT;
    };
    Price {
        input: row.0,
        cache_read: row.1,
        cache_write: row.2,
        output: row.3,
        window: row.4,
    }
}

/// 서버에 올려도 되는 모델 이름: 기본 가격표의 계열 이름, 아니면 "other".
/// 사용자가 덮어쓴 가격은 세지 않는다 — 사내 배포 이름은 기기를 떠나지 않는다.
/// ponytail: 계열 단위라 세부 버전은 합쳐진다. 프로바이더 사전(별도 스펙)이 생기면 표준 모델 ID로 바꾼다.
pub fn public_model(raw: &str) -> &'static str {
    let last = raw.rsplit('/').next().unwrap_or(raw);
    family_row(last).map(|(family, _)| family).unwrap_or("other")
}

fn config_dir() -> PathBuf {
    if let Ok(path) = std::env::var("XDG_CONFIG_HOME") {
        if !path.trim().is_empty() {
            return PathBuf::from(path).join("tokenmeter");
        }
    }
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".config/tokenmeter")
}

fn overrides() -> HashMap<String, HashMap<String, f64>> {
    let Ok(text) = fs::read_to_string(config_dir().join("prices.json")) else {
        return HashMap::new();
    };
    let Ok(Value::Object(book)) = serde_json::from_str::<Value>(&text) else {
        return HashMap::new();
    };
    book.into_iter()
        .filter_map(|(name, row)| {
            let row = row.as_object()?;
            let clean = ["input", "cache_read", "cache_write", "output", "window"]
                .into_iter()
                .filter_map(|field| {
                    row.get(field)
                        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
                        .map(|v| (field.to_string(), v))
                })
                .collect::<HashMap<_, _>>();
            (!clean.is_empty()).then(|| (key(&name), clean))
        })
        .collect()
}

fn price(model: &str) -> Price {
    let mut out = built_in(model);
    if let Some(row) = overrides().get(&key(model)) {
        out.input = row.get("input").copied().unwrap_or(out.input);
        out.cache_read = row.get("cache_read").copied().unwrap_or(out.cache_read);
        out.cache_write = row.get("cache_write").copied().unwrap_or(out.cache_write);
        out.output = row.get("output").copied().unwrap_or(out.output);
        out.window = row.get("window").copied().unwrap_or(out.window as f64) as i64;
    }
    out
}

pub fn cost_usd(model: &str, input: i64, cache_read: i64, cache_write: i64, output: i64) -> f64 {
    let p = price(model);
    (input as f64 * p.input
        + cache_read as f64 * p.cache_read
        + cache_write as f64 * p.cache_write
        + output as f64 * p.output)
        / 1_000_000.0
}

pub fn cache_savings(model: &str, cache_read: i64) -> f64 {
    let p = price(model);
    (cache_read as f64 * (p.input - p.cache_read).max(0.0)) / 1_000_000.0
}

pub fn context_window(model: &str, observed: i64) -> i64 {
    let n = key(model);
    let window = price(model).window;
    if n.contains("[1m]") || n.ends_with("-1m") || (window > 0 && observed > window) {
        1_000_000
    } else {
        window
    }
}

pub fn known(model: &str) -> bool {
    overrides().contains_key(&key(model)) || built_in(model).window > 0
}

pub fn prices_for(model: &str) -> Price {
    price(model)
}

pub fn has_override(model: &str) -> bool {
    overrides().contains_key(&key(model))
}

pub fn set_price(model: &str, values: &HashMap<String, f64>) -> HashMap<String, f64> {
    let mut book = overrides();
    let entry = book.entry(key(model)).or_default();
    for (field, value) in values {
        entry.insert(field.clone(), *value);
    }
    let out = entry.clone();
    save_overrides(&book);
    out
}

pub fn unset_price(model: &str) -> bool {
    let mut book = overrides();
    let gone = book.remove(&key(model)).is_some();
    if gone {
        save_overrides(&book);
    }
    gone
}

fn save_overrides(book: &HashMap<String, HashMap<String, f64>>) {
    let path = config_dir().join("prices.json");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut obj = serde_json::Map::new();
    for (name, row) in book {
        let mut inner = serde_json::Map::new();
        for (k, v) in row {
            inner.insert(k.clone(), json!(*v));
        }
        obj.insert(name.clone(), Value::Object(inner));
    }
    let _ = fs::write(path, Value::Object(obj).to_string());
}

pub fn listed_models() -> Vec<String> {
    [
        "claude-fable-5.1",
        "claude-fable-5",
        "claude-opus-5",
        "claude-opus-4.8",
        "claude-sonnet-5",
        "claude-sonnet-4.6",
        "claude-haiku-4.5",
        "gpt-5.6-sol",
        "gpt-5.6-terra",
        "gpt-5.6-luna",
        "gpt-5.4",
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        "grok-4.6-build",
        "grok-4.5",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

pub fn vendor_of(model: &str) -> String {
    let n = key(model);
    for (part, vendor) in [
        ("claude", "anthropic"),
        ("gpt", "openai"),
        ("o3-", "openai"),
        ("o4-", "openai"),
        ("codex", "openai"),
        ("gemini", "google"),
        ("deepseek", "deepseek"),
        ("grok", "xai"),
        ("llama", "meta"),
        ("mistral", "mistral"),
        ("mixtral", "mistral"),
        ("qwen", "alibaba"),
        ("nemotron", "nvidia"),
        ("command-r", "cohere"),
        ("kimi", "moonshot"),
    ] {
        if n.contains(part) {
            return vendor.into();
        }
    }
    "unknown".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_prices_match_python_contract() {
        assert_eq!(vendor_of("gpt-5.6-sol"), "openai");
        assert_eq!(context_window("claude-opus-5", 10), 1_000_000);
        assert!((cost_usd("claude-fable-5.1", 0, 1_000_000, 0, 0) - 0.25).abs() < 1e-9);
        assert!((cost_usd("claude-opus-5", 1_000_000, 0, 0, 0) - 5.0).abs() < 1e-9);
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn table_prices_each_token_kind_and_user_overrides_win() {
        let (_g, _tmp) = crate::test_home("prices");
        assert!(close(cost_usd("claude-opus-5", 0, 1_000_000, 0, 0), 0.5));
        assert!(close(cost_usd("claude-opus-5", 0, 0, 1_000_000, 0), 6.25));
        assert!(close(cost_usd("claude-opus-5", 0, 0, 0, 1_000_000), 25.0));
        assert!(close(cost_usd("모르는-모델-9", 1_000_000, 0, 0, 0), 3.0), "모르면 default 단가");
        let want = (2.0 * 5.0 + 60955.0 * 0.5 + 2161.0 * 6.25 + 813.0 * 25.0) / 1_000_000.0;
        assert!((cost_usd("claude-opus-5", 2, 60955, 2161, 813) - want).abs() < 1e-12);

        assert!(!known("nemotron-3-ultra"), "가격표에 없어야 하는 전제가 깨졌다");
        assert!(close(cost_usd("nemotron-3-ultra", 1_000_000, 0, 0, 0), 3.0));
        assert_eq!(context_window("nemotron-3-ultra", 0), 0, "모르면 0 — ctx% 를 지어내지 않는다");
        let set = |model: &str, pairs: &[(&str, f64)]| {
            set_price(model, &pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect());
        };
        set("nemotron-3-ultra", &[("input", 1.0), ("output", 2.0), ("window", 128_000.0)]);
        assert!(known("nemotron-3-ultra"));
        assert!(close(cost_usd("nemotron-3-ultra", 1_000_000, 0, 0, 0), 1.0));
        assert!(close(cost_usd("nemotron-3-ultra", 0, 0, 0, 1_000_000), 2.0));
        assert_eq!(context_window("nemotron-3-ultra", 0), 128_000);
        assert!(close(prices_for("nemotron-3-ultra").cache_read, 0.3), "안 적은 항목은 기본 표에서 온다");
        assert!(known("Nemotron_3-Ultra"), "대소문자/구분자가 달라도 같은 모델");

        set("claude-opus-5[1m]", &[("input", 10.0)]);
        assert!(close(cost_usd("claude-opus-5[1m]", 1_000_000, 0, 0, 0), 10.0));
        assert_eq!(context_window("claude-opus-5[1m]", 0), 1_000_000);
        assert!(close(cost_usd("claude-opus-5", 1_000_000, 0, 0, 0), 5.0), "본체 단가는 그대로");

        fs::write(config_dir().join("prices.json"), "{ 깨진").unwrap();
        assert!(close(cost_usd("claude-opus-5", 1_000_000, 0, 0, 0), 5.0), "깨진 파일은 무시한다");

        set("nemotron-3-ultra", &[("input", 1.0)]);
        assert!(unset_price("nemotron-3-ultra"));
        assert!(!unset_price("nemotron-3-ultra"));
        assert!(close(cost_usd("nemotron-3-ultra", 1_000_000, 0, 0, 0), 3.0));
    }

    #[test]
    fn public_model_reports_built_in_families_only() {
        let (_g, _tmp) = crate::test_home("public-model");
        assert_eq!(public_model("claude-opus-5-5"), "claude-opus-5");
        assert_eq!(
            public_model("arn:aws:bedrock:us-east-1:123456789012:application-inference-profile/us.anthropic.claude-sonnet-5-v1:0"),
            "claude-sonnet-5"
        );
        assert_eq!(public_model("openrouter/anthropic/claude-haiku-4.5"), "claude-haiku-4.5");
        assert_eq!(public_model("gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(public_model("acme-internal-llm"), "other");
        let dir = std::path::PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap()).join("tokenmeter");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("prices.json"), r#"{"acme-internal-llm": {"input": 1, "output": 2}}"#).unwrap();
        assert!(known("acme-internal-llm"), "사용자 가격이 있으면 로컬에서는 아는 모델");
        assert_eq!(public_model("acme-internal-llm"), "other", "그래도 이름은 밖으로 안 나간다");
    }
}
