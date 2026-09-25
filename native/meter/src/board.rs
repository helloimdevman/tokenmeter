//! 선택형 자체 호스팅 랭킹.

use crate::attention::attention_counts;
use crate::watch::{now_secs, setting_f64, setting_list, setting_map, setting_str};
use serde_json::{json, Map, Value};
use std::fs;
use tokenmeter_hook::data_dir;

const PUBLIC_HOSTS: &[&str] = &[
    "api.anthropic.com",
    "api.openai.com",
    "chatgpt.com",
    "generativelanguage.googleapis.com",
    "api.x.ai",
    "cli-chat-proxy.grok.com",
    "api.deepseek.com",
    "api.mistral.ai",
    "api.groq.com",
    "openrouter.ai",
    "api.together.xyz",
    "api.fireworks.ai",
    "api.moonshot.cn",
    "api.cohere.com",
    "api.perplexity.ai",
    "integrate.api.nvidia.com",
    "api.studio.nebius.ai",
    "opencode.ai",
];

#[derive(Clone)]
pub struct Entry {
    pub handle: String,
    pub tokens: i64,
    pub cost_usd: f64,
    pub me: bool,
}

#[derive(Clone)]
pub struct TeamEntry {
    pub handle: String,
    pub check: i64,
    pub working: i64,
    pub waiting: i64,
    pub risk: i64,
    pub cost_usd: f64,
    pub me: bool,
}

fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn totals_of(node: &Value) -> &Value {
    node.get("totals").unwrap_or(node)
}

pub fn tokens_of(node: &Value) -> i64 {
    let t = totals_of(node);
    ["input_tokens", "cache_read", "cache_write", "output_tokens"]
        .into_iter()
        .map(|k| num(t.get(k)) as i64)
        .sum()
}

fn cost_of(node: &Value) -> f64 {
    num(totals_of(node).get("cost_usd")).max(0.0)
}

fn host_of(url: &str) -> String {
    let text = url.trim();
    if text.is_empty() {
        return String::new();
    }
    if matches!(text, "bedrock" | "vertex") {
        return text.into();
    }
    let with = if text.contains("://") {
        text.to_string()
    } else {
        format!("https://{text}")
    };
    with.split("://")
        .nth(1)
        .unwrap_or(&with)
        .split('/')
        .next()
        .unwrap_or("")
        .split('@')
        .next_back()
        .unwrap_or("")
        .to_ascii_lowercase()
}

pub(crate) fn endpoint_label(url: &str) -> String {
    classify(url, &setting_list(&["settings", "leaderboard", "public_endpoints"]))
}

/// 리그 통계용 경로 라벨. 사용자 `public_endpoints`는 레거시 리더보드에만 쓰고 여기서는 무시한다.
pub(crate) fn public_label(url: &str) -> String {
    classify(url, &[])
}

fn classify(url: &str, extra: &[String]) -> String {
    let host = host_of(url);
    if host.is_empty() || host == "unknown" {
        return "unknown".into();
    }
    if matches!(host.as_str(), "bedrock" | "vertex") {
        return host;
    }
    if PUBLIC_HOSTS.contains(&host.as_str()) || extra.iter().any(|h| h == &host) {
        return host;
    }
    for (suffix, label) in [
        (".openai.azure.com", "azure-openai"),
        (".azure-api.net", "azure-openai"),
        ("-aiplatform.googleapis.com", "vertex"),
        (".amazonaws.com", "bedrock"),
    ] {
        if host.ends_with(suffix) {
            return label.into();
        }
    }
    "self-hosted".into()
}

fn bucket(node: &Value) -> Map<String, Value> {
    let t = totals_of(node);
    let mut out = Map::new();
    for k in ["input_tokens", "cache_read", "cache_write", "output_tokens", "calls"] {
        out.insert(k.into(), json!(num(t.get(k)) as i64));
    }
    out.insert("cost_usd".into(), json!((cost_of(node) * 1e8).round() / 1e8));
    out
}

fn group(state: &Value, name: &str, cap: usize) -> Value {
    let mut items: Vec<(String, Value)> = state
        .get(name)
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    items.sort_by(|a, b| cost_of(&b.1).partial_cmp(&cost_of(&a.1)).unwrap_or(std::cmp::Ordering::Equal));
    let mut out = Map::new();
    for (key, node) in items.into_iter().take(cap) {
        let mut entry = bucket(&node);
        entry.insert("sessions".into(), json!(num(node.get("sessions")) as i64));
        if let Some(v) = node.get("vendor") {
            entry.insert("vendor".into(), v.clone());
        }
        out.insert(key, Value::Object(entry));
    }
    Value::Object(out)
}

fn endpoints(state: &Value, public: &[String]) -> Value {
    let mut out: std::collections::BTreeMap<String, Map<String, Value>> = Default::default();
    if let Some(raw) = state.get("endpoints").and_then(Value::as_object) {
        for (url, node) in raw {
            let label = classify(url, public);
            let mut entry = bucket(node);
            entry.insert("sessions".into(), json!(num(node.get("sessions")) as i64));
            let slot = out.entry(label).or_insert_with(|| {
                let mut z = Map::new();
                for k in ["input_tokens", "cache_read", "cache_write", "output_tokens", "calls", "sessions"] {
                    z.insert(k.into(), json!(0));
                }
                z.insert("cost_usd".into(), json!(0.0));
                z
            });
            for (k, v) in entry {
                let add = v.as_f64().or_else(|| v.as_i64().map(|n| n as f64)).unwrap_or(0.0);
                let cur = slot.get(&k).and_then(Value::as_f64).unwrap_or(0.0);
                if k == "cost_usd" {
                    slot.insert(k, json!(((cur + add) * 1e8).round() / 1e8));
                } else {
                    slot.insert(k, json!((cur + add) as i64));
                }
            }
        }
    }
    Value::Object(out.into_iter().map(|(k, v)| (k, Value::Object(v))).collect())
}

fn handle() -> String {
    let h = setting_str(&["settings", "leaderboard", "handle"]);
    if !h.is_empty() {
        return h;
    }
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "me".into())
}

fn sessions_today(state: &Value) -> i64 {
    state
        .get("sessions")
        .and_then(Value::as_object)
        .map(|book| book.len() as i64)
        .unwrap_or(0)
}

pub fn payload(state: &Value) -> Value {
    let public = setting_list(&["settings", "leaderboard", "public_endpoints"]);
    let now = now_secs();
    let (check, working, waiting, risk) = attention_counts(state, now);
    let mut today = bucket(state.get("today").unwrap_or(&json!({})));
    today.insert(
        "date".into(),
        json!(state.pointer("/today/date").and_then(Value::as_str).unwrap_or("")),
    );
    today.insert("sessions".into(), json!(sessions_today(state)));
    today.insert(
        "attention".into(),
        json!({"check": check, "working": working, "waiting": waiting, "risk": risk}),
    );
    let mut total = bucket(state.get("total").unwrap_or(&json!({})));
    total.insert(
        "sessions".into(),
        json!(num(state.pointer("/total/sessions")) as i64),
    );
    json!({
        "handle": handle(),
        "updated_at": now,
        "today": today,
        "total": total,
        "models": group(state, "models", 40),
        "vendors": group(state, "vendors", 20),
        "plans": group(state, "plans", 10),
        "clients": group(state, "services", 20),
        "endpoints": endpoints(state, &public),
    })
}

fn rank(mut rows: Vec<Entry>) -> Vec<Entry> {
    rows.sort_by(|a, b| {
        b.cost_usd
            .partial_cmp(&a.cost_usd)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.tokens.cmp(&a.tokens))
            .then(a.handle.cmp(&b.handle))
    });
    rows
}

fn parse_entries(raw: &Value, scope: &str, me: &str) -> Vec<Entry> {
    let rows = raw.get("entries").unwrap_or(raw);
    let Some(list) = rows.as_array() else {
        return Vec::new();
    };
    rank(
        list.iter()
            .filter_map(|row| {
                let handle = row.get("handle").and_then(Value::as_str)?.trim().to_string();
                if handle.is_empty() {
                    return None;
                }
                let node = row.get(scope).unwrap_or(row.get("total").unwrap_or(&Value::Null));
                Some(Entry {
                    me: handle == me,
                    tokens: tokens_of(node),
                    cost_usd: cost_of(node),
                    handle,
                })
            })
            .collect(),
    )
}

fn parse_team(raw: &Value, me: &str) -> Vec<TeamEntry> {
    let rows = raw.get("entries").unwrap_or(raw);
    let mut out = Vec::new();
    for row in rows.as_array().into_iter().flatten() {
        let handle = row.get("handle").and_then(Value::as_str).unwrap_or("").trim();
        if handle.is_empty() {
            continue;
        }
        let today = row.get("today").unwrap_or(&Value::Null);
        let att = today.get("attention").unwrap_or(&Value::Null);
        out.push(TeamEntry {
            handle: handle.into(),
            check: num(att.get("check")) as i64,
            working: num(att.get("working")) as i64,
            waiting: num(att.get("waiting")) as i64,
            risk: num(att.get("risk")) as i64,
            cost_usd: cost_of(today),
            me: handle == me,
        });
    }
    out.sort_by(|a, b| {
        b.check
            .cmp(&a.check)
            .then(b.risk.cmp(&a.risk))
            .then(b.working.cmp(&a.working))
            .then(b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.handle.cmp(&b.handle))
    });
    out
}

fn cache_path() -> std::path::PathBuf {
    data_dir().join("leaderboard.json")
}

fn cached() -> Value {
    fs::read_to_string(cache_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(json!({}))
}

fn write_cache(entries: Option<Value>, status: &str) {
    let prev = cached();
    let data = json!({
        "fetched_at": if entries.is_some() { now_secs() } else { num(prev.get("fetched_at")) },
        "entries": entries.unwrap_or_else(|| prev.get("entries").cloned().unwrap_or(json!([]))),
        "status": status,
        "handle": handle(),
    });
    let _ = fs::create_dir_all(data_dir());
    let tmp = cache_path().with_extension("tmp");
    if fs::write(&tmp, serde_json::to_string_pretty(&data).unwrap_or_default()).is_ok() {
        let _ = fs::rename(tmp, cache_path());
    }
}

pub fn online() -> bool {
    !setting_str(&["settings", "leaderboard", "endpoint"]).is_empty()
}

pub fn sync(state: &Value, force: bool) {
    let endpoint = setting_str(&["settings", "leaderboard", "endpoint"]);
    if endpoint.is_empty() {
        return;
    }
    let _ = force;
    let body = payload(state).to_string();
    let headers = setting_map(&["settings", "leaderboard", "headers"]);
    let post = || {
        let mut req = ureq::post(&endpoint)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json");
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        req.timeout(std::time::Duration::from_secs(4))
            .send_string(&body)
            .ok()?;
        let mut get = ureq::get(&endpoint).set("Accept", "application/json");
        for (k, v) in &headers {
            get = get.set(k, v);
        }
        get.timeout(std::time::Duration::from_secs(4))
            .call()
            .ok()?
            .into_json::<Value>()
            .ok()
    };
    match post() {
        Some(raw) => {
            let rows = raw.get("entries").cloned().unwrap_or(raw);
            write_cache(Some(if rows.is_array() { rows } else { json!([]) }), "");
        }
        None => write_cache(None, "동기화 실패: URLError"),
    }
}

pub fn board(state: &Value, scope: &str) -> (Vec<Entry>, String) {
    let me = handle();
    let mine = Entry {
        handle: me.clone(),
        tokens: tokens_of(state.get(if scope == "today" { "today" } else { "total" }).unwrap_or(&json!({}))),
        cost_usd: cost_of(state.get(if scope == "today" { "today" } else { "total" }).unwrap_or(&json!({}))),
        me: true,
    };
    let cache = cached();
    let mut entries = parse_entries(&cache, scope, &me);
    if !entries.iter().any(|e| e.me) {
        entries = rank({
            entries.push(mine);
            entries
        });
    } else {
        entries = rank(
            entries
                .into_iter()
                .map(|e| if e.me { mine.clone() } else { e })
                .collect(),
        );
    }
    if !online() {
        return (entries, "혼자 달리는 중 · endpoint 를 채우면 참가".into());
    }
    let status = cache.get("status").and_then(Value::as_str).unwrap_or("");
    if !status.is_empty() {
        return (entries, status.into());
    }
    let n = entries.len();
    (entries, format!("동기화 · {n}명"))
}

pub fn team(state: &Value) -> (Vec<TeamEntry>, String) {
    let me = handle();
    let now = now_secs();
    let (check, working, waiting, risk) = attention_counts(state, now);
    let mine = TeamEntry {
        handle: me.clone(),
        check,
        working,
        waiting,
        risk,
        cost_usd: cost_of(state.get("today").unwrap_or(&json!({}))),
        me: true,
    };
    if !online() {
        return (vec![mine], "혼자 달리는 중 · endpoint 를 채우면 참가".into());
    }
    let cache = cached();
    let mut entries = parse_team(&cache, &me);
    if !entries.iter().any(|e| e.me) {
        entries.push(mine);
    } else {
        entries = entries
            .into_iter()
            .map(|e| if e.me { mine.clone() } else { e })
            .collect();
    }
    let status = cache.get("status").and_then(Value::as_str).unwrap_or("");
    if !status.is_empty() {
        return (entries, status.into());
    }
    let n = entries.len();
    (entries, format!("동기화 · {n}명"))
}

pub fn sync_seconds() -> f64 {
    setting_f64(&["settings", "leaderboard", "sync_seconds"], 60.0)
}
