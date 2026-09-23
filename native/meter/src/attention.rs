//! 라이브 신호 + 토큰 시각 → 확인/작업/대기/종료.

use serde_json::{Map, Value};
use std::collections::BTreeMap;
use tokenmeter_hook::project_key;

pub const ATTENTION_ACTIVE_SECONDS: f64 = 30.0;
pub const ATTENTION_LABELS: &[(&str, &str)] = &[
    ("check", "확인"),
    ("working", "작업"),
    ("waiting", "대기"),
    ("done", "종료"),
];

pub fn attention_label(kind: &str) -> &'static str {
    ATTENTION_LABELS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, label)| *label)
        .unwrap_or("?")
}

fn as_f64(v: Option<&Value>) -> f64 {
    let n = match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    };
    if n.is_finite() {
        n
    } else {
        0.0
    }
}

fn as_i64(v: Option<&Value>) -> i64 {
    as_f64(v) as i64
}

fn as_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string().trim_matches('"').to_string(),
        None => String::new(),
    }
}

#[derive(Clone, Debug)]
pub struct SessionView {
    pub key: String,
    pub service: String,
    pub session_id: String,
    pub project: String,
    pub attention: String,
    pub attention_at: f64,
    pub live: bool,
    pub last_seen: f64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub main_output_tokens: i64,
    pub ctx: i64,
    pub ctx_win: i64,
    pub model: String,
    pub started_at: f64,
    pub cwd: String,
    pub effort: String,
    pub provider: String,
    pub event: String,
    pub cost_usd: f64,
    pub sub_cost: f64,
}

/// 저장된 세션 + 라이브 파일을 합쳐 주의 상태를 낸다. Python `session_views` 와 같다.
pub fn session_views(state: &Value, now: f64) -> Vec<SessionView> {
    let stored = state
        .get("sessions")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let live_rows = state.get("live").and_then(Value::as_array);
    let mut live: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    if let Some(rows) = live_rows {
        for row in rows {
            if let Some(obj) = row.as_object() {
                let service = as_str(obj.get("service"));
                let sid = as_str(obj.get("session_id"));
                let key = format!(
                    "{}/{}",
                    if service.is_empty() { "?" } else { &service },
                    if sid.is_empty() { "?" } else { &sid }
                );
                live.insert(key, obj.clone());
            }
        }
    }
    let mut keys: Vec<String> = stored.keys().cloned().collect();
    for k in live.keys() {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let mut rows = Vec::new();
    for key in keys {
        let rec = stored.get(&key).and_then(Value::as_object);
        let active = live.get(&key);
        let token_at = as_f64(rec.and_then(|r| r.get("last_seen")));
        let signal = active
            .map(|a| as_str(a.get("attention")))
            .unwrap_or_default();
        let signal_at =
            as_f64(active.and_then(|a| a.get("attention_at").or_else(|| a.get("updated_at"))));
        let event = active
            .map(|a| as_str(a.get("event")))
            .filter(|s| !s.is_empty())
            .or_else(|| rec.map(|r| as_str(r.get("event"))))
            .unwrap_or_default();
        let mut signal = signal;
        if signal == "check" && (event == "Stop" || event == "session.idle") {
            signal = "waiting".into();
        }
        let started_at = as_f64(
            rec.and_then(|r| r.get("started_at"))
                .or_else(|| active.and_then(|a| a.get("started_at"))),
        );
        let model = active
            .map(|a| as_str(a.get("model")))
            .filter(|s| !s.is_empty())
            .or_else(|| rec.map(|r| as_str(r.get("model"))))
            .unwrap_or_default();
        let effort = active
            .map(|a| as_str(a.get("effort")))
            .filter(|s| !s.is_empty())
            .or_else(|| rec.map(|r| as_str(r.get("effort"))))
            .unwrap_or_default();
        let provider = active
            .map(|a| as_str(a.get("provider")))
            .filter(|s| !s.is_empty())
            .or_else(|| rec.map(|r| as_str(r.get("provider"))))
            .unwrap_or_default();
        let attention = if active.is_none() {
            "done"
        } else if signal == "check" && signal_at > 0.0 && signal_at >= token_at {
            "check"
        } else if now - token_at.max(if signal == "working" { signal_at } else { 0.0 })
            <= ATTENTION_ACTIVE_SECONDS
        {
            "working"
        } else {
            "waiting"
        };
        let (service, session_id) = key.split_once('/').unwrap_or(("?", key.as_str()));
        let cwd = active
            .map(|a| as_str(a.get("cwd")))
            .filter(|s| !s.is_empty())
            .or_else(|| rec.map(|r| as_str(r.get("cwd"))))
            .unwrap_or_default();
        let project = {
            let from_cwd = project_key(&cwd);
            if !from_cwd.is_empty() {
                from_cwd
            } else {
                active
                    .map(|a| as_str(a.get("project")))
                    .filter(|s| !s.is_empty())
                    .or_else(|| rec.map(|r| as_str(r.get("project"))))
                    .unwrap_or_else(|| "(unknown)".into())
            }
        };
        let totals = rec.and_then(|r| r.get("totals")).and_then(Value::as_object);
        let output = as_i64(totals.and_then(|t| t.get("output_tokens")));
        let total_tokens = ["input_tokens", "cache_read", "cache_write", "output_tokens"]
            .into_iter()
            .map(|name| as_i64(totals.and_then(|t| t.get(name))))
            .sum();
        let sub = as_i64(rec.and_then(|r| r.get("sub_output_tokens")));
        rows.push(SessionView {
            key: key.clone(),
            service: rec
                .and_then(|r| r.get("service"))
                .and_then(Value::as_str)
                .or_else(|| {
                    active
                        .and_then(|a| a.get("service"))
                        .and_then(Value::as_str)
                })
                .unwrap_or(service)
                .to_string(),
            session_id: active
                .and_then(|a| a.get("session_id"))
                .and_then(Value::as_str)
                .unwrap_or(session_id)
                .to_string(),
            project,
            attention: attention.into(),
            attention_at: signal_at,
            live: active.is_some(),
            last_seen: token_at,
            output_tokens: output,
            total_tokens,
            main_output_tokens: (output - sub).max(0),
            ctx: as_i64(rec.and_then(|r| r.get("ctx"))),
            ctx_win: as_i64(rec.and_then(|r| r.get("ctx_win"))),
            model,
            started_at,
            cwd,
            effort,
            provider,
            event,
            cost_usd: as_f64(totals.and_then(|t| t.get("cost_usd"))),
            sub_cost: as_f64(rec.and_then(|r| r.get("sub_cost"))),
        });
    }
    let order = |a: &str| match a {
        "check" => 0,
        "working" => 1,
        "waiting" => 2,
        _ => 3,
    };
    rows.sort_by(|a, b| {
        order(&a.attention).cmp(&order(&b.attention)).then(
            b.last_seen
                .partial_cmp(&a.last_seen)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });
    rows
}

pub fn attention_counts(state: &Value, now: f64) -> (i64, i64, i64, i64) {
    let mut check = 0;
    let mut working = 0;
    let mut waiting = 0;
    let mut risk = 0;
    for row in session_views(state, now) {
        match row.attention.as_str() {
            "check" => {
                check += 1;
                risk += 1;
            }
            "working" => working += 1,
            "waiting" => waiting += 1,
            _ => {}
        }
    }
    (check, working, waiting, risk)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn views_merge_live_signal_with_token_time() {
        let now = 10_000.0;
        let state = json!({
            "sessions": {
                "claude-code/a": {"service": "claude-code", "project": "api", "last_seen": now - 5.0,
                                  "ctx": 180_000, "ctx_win": 200_000,
                                  "totals": {"output_tokens": 10, "cost_usd": 0.4}},
                "codex/b": {"service": "codex", "project": "web", "last_seen": now - 100.0,
                            "totals": {"output_tokens": 2}},
                "codex/done": {"service": "codex", "project": "old", "last_seen": now - 200.0,
                               "totals": {"output_tokens": 1}}
            },
            "live": [
                {"service": "claude-code", "session_id": "a", "attention": "check",
                 "attention_at": now - 10.0, "updated_at": now - 10.0,
                 "event": "PermissionRequest", "cwd": "/tmp/shop/api"},
                {"service": "codex", "session_id": "b", "attention": "check",
                 "attention_at": now - 1.0, "updated_at": now - 1.0},
                {"service": "opencode", "session_id": "c", "project": "docs", "attention": "working",
                 "attention_at": now - 40.0, "updated_at": now - 40.0}
            ]
        });
        let rows: std::collections::HashMap<String, SessionView> =
            session_views(&state, now).into_iter().map(|r| (r.key.clone(), r)).collect();
        assert_eq!(rows["claude-code/a"].attention, "working", "확인 뒤 토큰이 흐르면 작업이다");
        assert_eq!(rows["codex/b"].attention, "check");
        assert_eq!(rows["opencode/c"].attention, "waiting");
        assert_eq!(rows["codex/done"].attention, "done");
        assert_eq!(attention_counts(&state, now), (1, 1, 1, 1));
        let a = &rows["claude-code/a"];
        assert_eq!((a.event.as_str(), a.cwd.as_str(), a.cost_usd), ("PermissionRequest", "/tmp/shop/api", 0.4));

        let stale_stop = json!({
            "sessions": {"codex/stop": {"last_seen": now - 100.0, "totals": {}}},
            "live": [{"service": "codex", "session_id": "stop", "event": "Stop",
                      "attention": "check", "attention_at": now - 1.0}]
        });
        assert_eq!(session_views(&stale_stop, now)[0].attention, "waiting");
        assert_eq!(attention_counts(&stale_stop, now).0, 0);
    }

    #[test]
    fn malformed_numbers_degrade_to_zero() {
        let state = json!({
            "sessions": {
                "broken-key": {"project": {"private": "value"}, "started_at": {}, "last_seen": 1,
                               "ctx": [], "ctx_win": "Infinity", "sub_cost": [], "totals": ["broken"]},
                "codex/live": {"service": "codex", "project": "api", "model": "gpt",
                               "started_at": "NaN", "last_seen": {}, "ctx": "-Infinity",
                               "ctx_win": "inf", "totals": {"input_tokens": {}, "cost_usd": "NaN"}}
            },
            "live": [{"service": "codex", "session_id": "live", "attention": "working",
                      "attention_at": "Infinity", "started_at": []}]
        });
        let rows: std::collections::HashMap<String, SessionView> =
            session_views(&state, 10.0).into_iter().map(|r| (r.key.clone(), r)).collect();
        assert_eq!(rows["broken-key"].attention, "done");
        assert_eq!(rows["broken-key"].ctx_win, 0);
        let live = &rows["codex/live"];
        assert!(live.live);
        assert_eq!((live.last_seen, live.started_at, live.attention_at), (0.0, 0.0, 0.0));
        assert_eq!((live.ctx, live.ctx_win, live.cost_usd), (0, 0, 0.0));
    }
}
