//! 프로바이더 한도. 자격 증명이 있으면 네트워크로 읽고 quota.json 에 쓴다.

use crate::watch::now_secs;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tokenmeter_hook::data_dir;

const TTL: f64 = 180.0;
const WARN: f64 = 0.70;
const HOT: f64 = 0.90;
const CLAUDE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const CODEX_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const GROK_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";

pub fn quota_path() -> PathBuf {
    data_dir().join("quota.json")
}

pub fn load() -> Value {
    fs::read_to_string(quota_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(|v: &Value| v.is_object())
        .unwrap_or_else(|| json!({"updated_at": 0.0, "windows": [], "errors": {}}))
}

fn save(snap: &Value) {
    let _ = fs::create_dir_all(data_dir());
    let path = quota_path();
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, snap.to_string()).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}

pub fn public(snap: &Value) -> Value {
    json!({
        "updated_at": snap.get("updated_at").and_then(Value::as_f64).unwrap_or(0.0),
        "windows": snap.get("windows").cloned().unwrap_or(json!([])),
        "errors": snap.get("errors").cloned().unwrap_or(json!({})),
    })
}

pub fn due(snap: &Value, now: f64) -> bool {
    let age = now - snap.get("updated_at").and_then(Value::as_f64).unwrap_or(0.0);
    age < 0.0 || age >= TTL
}

pub fn refresh(force: bool) -> Value {
    let now = now_secs();
    let prev = load();
    if !force && !due(&prev, now) {
        return prev;
    }
    let homes = homes();
    let mut found: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    let mut errors = serde_json::Map::new();
    for (name, fetch) in [
        ("claude-code", fetch_claude as fn(f64, &Homes) -> Result<Vec<Value>, String>),
        ("codex", fetch_codex),
        ("grok", fetch_grok),
    ] {
        match fetch(now, &homes) {
            Ok(rows) => {
                found.insert(name.into(), rows);
            }
            Err(err) => {
                errors.insert(name.into(), json!(err));
                let old: Vec<Value> = prev
                    .get("windows")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|row| row.get("source").and_then(Value::as_str) == Some(name))
                    .map(|row| {
                        let mut stale = row.clone();
                        if let Some(obj) = stale.as_object_mut() {
                            obj.insert("status".into(), json!("stale"));
                        }
                        stale
                    })
                    .collect();
                if !old.is_empty() {
                    found.insert(name.into(), old);
                }
            }
        }
    }
    let mut windows = Vec::new();
    for name in ["claude-code", "codex", "grok"] {
        windows.extend(found.remove(name).unwrap_or_default());
    }
    let snap = json!({"updated_at": now, "windows": windows, "errors": errors});
    save(&snap);
    snap
}

struct Homes {
    claude: PathBuf,
    codex: PathBuf,
    grok: PathBuf,
}

fn homes() -> Homes {
    let home = dirs_home();
    let claude = std::env::var("CLAUDE_CONFIG_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|p| crate::watch::expand_home(&p).join(".credentials.json"))
        .unwrap_or_else(|| home.join(".claude/.credentials.json"));
    Homes {
        claude,
        codex: home.join(".codex/auth.json"),
        grok: home.join(".grok/auth.json"),
    }
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn get_json(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let mut req = ureq::get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let resp = req.timeout(std::time::Duration::from_secs(8)).call();
    match resp {
        Ok(r) => r
            .into_json()
            .map_err(|_| "invalid json".into())
            .and_then(|v: Value| {
                if v.is_object() {
                    Ok(v)
                } else {
                    Err("unexpected payload".into())
                }
            }),
        Err(ureq::Error::Status(code, _)) => Err(code.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|_| format!("{} 없음", path.display()))?;
    serde_json::from_str::<Value>(&text)
        .map_err(|_| format!("{} 없음", path.file_name().unwrap_or_default().to_string_lossy()))
}

fn extract_claude(data: &Value) -> String {
    data.pointer("/claudeAiOauth/accessToken")
        .and_then(Value::as_str)
        .or_else(|| data.get("accessToken").and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

fn claude_token(homes: &Homes) -> Result<String, String> {
    if homes.claude.is_file() {
        let token = extract_claude(&read_json(&homes.claude)?);
        return if token.is_empty() {
            Err("claude 자격 없음".into())
        } else {
            Ok(token)
        };
    }
    if cfg!(target_os = "macos") {
        if let Ok(out) = Command::new("security")
            .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
            .output()
        {
            if out.status.success() {
                if let Ok(data) = serde_json::from_slice::<Value>(&out.stdout) {
                    let token = extract_claude(&data);
                    if !token.is_empty() {
                        return Ok(token);
                    }
                }
            }
        }
    }
    Err("claude 자격 없음".into())
}

fn fetch_claude(now: f64, homes: &Homes) -> Result<Vec<Value>, String> {
    let token = claude_token(homes)?;
    let data = get_json(
        CLAUDE_URL,
        &[
            ("Authorization", &format!("Bearer {token}")),
            ("anthropic-beta", "oauth-2025-04-20"),
        ],
    )?;
    Ok(parse_claude(&data, now))
}

fn fetch_codex(now: f64, homes: &Homes) -> Result<Vec<Value>, String> {
    let data = read_json(&homes.codex)?;
    let tokens = data.get("tokens").and_then(Value::as_object);
    let token = tokens
        .and_then(|t| t.get("access_token"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if token.is_empty() {
        return Err("codex 자격 없음".into());
    }
    let account = tokens
        .and_then(|t| t.get("account_id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let mut headers = vec![("Authorization", format!("Bearer {token}"))];
    if !account.is_empty() {
        headers.push(("ChatGPT-Account-Id", account.to_string()));
    }
    let refs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    Ok(parse_codex(&get_json(CODEX_URL, &refs)?, now))
}

fn fetch_grok(now: f64, homes: &Homes) -> Result<Vec<Value>, String> {
    let data = read_json(&homes.grok)?;
    let mut rec = None;
    if let Some(obj) = data.as_object() {
        for (key, item) in obj {
            if item.get("key").and_then(Value::as_str).unwrap_or("").is_empty() {
                continue;
            }
            rec = Some(item);
            if key.contains("auth.x.ai") {
                break;
            }
        }
    }
    let rec = rec.ok_or_else(|| "grok 자격 없음".to_string())?;
    let token = rec.get("key").and_then(Value::as_str).unwrap_or("");
    if token.is_empty() {
        return Err("grok 자격 없음".into());
    }
    if let Some(exp) = ts(rec.get("expires_at"), now) {
        if exp < now {
            return Err("grok 로그인 만료".into());
        }
    }
    Ok(parse_grok(
        &get_json(
            GROK_URL,
            &[
                ("Authorization", &format!("Bearer {token}")),
                ("x-xai-token-auth", "xai-grok-cli"),
                ("Accept", "application/json"),
            ],
        )?,
        now,
    ))
}

fn window(
    source: &str,
    kind: &str,
    label: &str,
    used: Option<f64>,
    reset: Option<&Value>,
    now: f64,
    remaining_usd: Option<f64>,
    cap_usd: Option<f64>,
    percent: bool,
    period: Option<f64>,
) -> Value {
    let ratio = ratio(used, percent);
    json!({
        "source": source,
        "title": title(source),
        "plan": "subscription",
        "kind": kind,
        "label": label,
        "used": ratio,
        "remaining_usd": remaining_usd,
        "cap_usd": cap_usd,
        "resets_at": ts(reset, now),
        "period_seconds": period.filter(|s| s.is_finite() && *s > 0.0),
        "status": status(ratio),
        "note": "",
        "fetched_at": now,
    })
}

fn title(source: &str) -> String {
    match source {
        "claude-code" => "Claude Code".into(),
        "codex" => "Codex".into(),
        "grok" => "Grok".into(),
        _ => source.into(),
    }
}

fn status(used: Option<f64>) -> &'static str {
    match used {
        Some(u) if u >= HOT => "exhausted",
        Some(u) if u >= WARN => "warn",
        _ => "ok",
    }
}

fn ratio(value: Option<f64>, percent: bool) -> Option<f64> {
    let mut num = value?;
    if percent || num > 1.0 {
        num /= 100.0;
    }
    Some(num.clamp(0.0, 1.0))
}

fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn ts(value: Option<&Value>, now: f64) -> Option<f64> {
    let value = value?;
    if let Some(num) = as_f64(Some(value)) {
        return Some(if num > 1e12 {
            num / 1000.0
        } else if num > 1e9 {
            num
        } else {
            now + num
        });
    }
    let mut text = value.as_str()?.trim().to_string();
    if text.ends_with('Z') {
        text = text.trim_end_matches('Z').to_string() + "+00:00";
    }
    chrono_parse(&text)
}

fn chrono_parse(text: &str) -> Option<f64> {
    // ponytail: RFC3339 만. 그 밖은 무시.
    let text = text.replace('T', " ");
    let secs = time_parse(&text)?;
    Some(secs)
}

fn time_parse(text: &str) -> Option<f64> {
    let cleaned = text
        .trim()
        .trim_end_matches('Z')
        .split('+')
        .next()?
        .trim()
        .replace('T', " ");
    let (date, time) = cleaned.split_once(' ')?;
    let mut d = date.split('-');
    let y: i32 = d.next()?.parse().ok()?;
    let m: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let h: u32 = t.next()?.parse().ok()?;
    let min: u32 = t.next()?.parse().ok()?;
    let s: f64 = t.next().unwrap_or("0").parse().ok()?;
    let days = days_from_civil(y, m, day)?;
    Some(days as f64 * 86400.0 + h as f64 * 3600.0 + min as f64 * 60.0 + s)
}

fn days_from_civil(y: i32, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || d == 0 || d > 31 {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era as i64 * 146097 + doe as i64 - 719468)
}

fn money(value: Option<&Value>, cents: bool) -> Option<f64> {
    let n = as_f64(value)?;
    Some(if cents { n / 100.0 } else { n })
}

fn cents_val(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Object(o) => as_f64(o.get("val")),
        other => as_f64(Some(other)),
    }
}

fn used_field(node: &Value) -> Option<f64> {
    for key in ["used_percent", "usedPercent", "utilization", "percent"] {
        if let Some(v) = as_f64(node.get(key)) {
            return Some(v);
        }
    }
    None
}

fn reset_field<'a>(node: &'a Value) -> Option<&'a Value> {
    for key in ["resets_at", "reset_at", "resetAt", "resetsAt"] {
        if node.get(key).is_some() {
            return node.get(key);
        }
    }
    for key in ["reset_after_seconds", "resets_in_seconds", "resetAfterSeconds"] {
        if node.get(key).is_some() {
            return node.get(key);
        }
    }
    None
}

fn parse_claude(data: &Value, now: f64) -> Vec<Value> {
    let mut rows = Vec::new();
    if let Some(limits) = data.get("limits").and_then(Value::as_array) {
        if !limits.is_empty() {
            for item in limits {
                let kind = item.get("kind").and_then(Value::as_str).unwrap_or("");
                let used = as_f64(item.get("percent"));
                let reset = item.get("resets_at");
                match kind {
                    "session" => rows.push(window(
                        "claude-code",
                        "session",
                        "5h",
                        used,
                        reset,
                        now,
                        None,
                        None,
                        true,
                        Some(5.0 * 3600.0),
                    )),
                    "weekly_all" => rows.push(window(
                        "claude-code",
                        "weekly",
                        "주간",
                        used,
                        reset,
                        now,
                        None,
                        None,
                        true,
                        Some(7.0 * 86400.0),
                    )),
                    "weekly_scoped" => {
                        let name = item
                            .pointer("/scope/model/display_name")
                            .and_then(Value::as_str)
                            .unwrap_or("모델");
                        rows.push(window(
                            "claude-code",
                            "weekly_scoped",
                            &format!("{name} 주"),
                            used,
                            reset,
                            now,
                            None,
                            None,
                            true,
                            Some(7.0 * 86400.0),
                        ));
                    }
                    _ => {}
                }
            }
        }
    } else {
        for (key, kind, label) in [
            ("five_hour", "session", "5h"),
            ("seven_day", "weekly", "주간"),
            ("seven_day_sonnet", "weekly_scoped", "Sonnet 주"),
            ("seven_day_opus", "weekly_scoped", "Opus 주"),
        ] {
            if let Some(node) = data.get(key).and_then(Value::as_object) {
                rows.push(window(
                    "claude-code",
                    kind,
                    label,
                    as_f64(node.get("utilization")),
                    node.get("resets_at"),
                    now,
                    None,
                    None,
                    true,
                    Some(if kind == "session" {
                        5.0 * 3600.0
                    } else {
                        7.0 * 86400.0
                    }),
                ));
            }
        }
    }
    if let Some(extra) = data.get("extra_usage").and_then(Value::as_object) {
        if extra.get("is_enabled") == Some(&json!(true)) {
            if let Some(cap) = money(extra.get("monthly_limit"), true) {
                let used = money(extra.get("used_credits"), true).unwrap_or(0.0);
                rows.push(window(
                    "claude-code",
                    "credits",
                    "추가사용",
                    if cap > 0.0 { Some(used / cap * 100.0) } else { None },
                    extra.get("resets_at"),
                    now,
                    Some((cap - used).max(0.0)),
                    Some(cap),
                    true,
                    None,
                ));
            }
        }
    }
    rows
}

fn parse_codex(data: &Value, now: f64) -> Vec<Value> {
    let limit = data
        .get("rate_limit")
        .filter(|v| v.is_object())
        .unwrap_or(data);
    let mut rows = Vec::new();
    if let Some(primary) = limit.get("primary_window").and_then(Value::as_object) {
        let node = Value::Object(primary.clone());
        rows.push(window(
            "codex",
            "session",
            &codex_label(&node, "5h"),
            used_field(&node),
            reset_field(&node),
            now,
            None,
            None,
            true,
            Some(codex_period(&node, 5.0 * 3600.0)),
        ));
    }
    if let Some(secondary) = limit.get("secondary_window").and_then(Value::as_object) {
        let node = Value::Object(secondary.clone());
        rows.push(window(
            "codex",
            "weekly",
            &codex_label(&node, "주간"),
            used_field(&node),
            reset_field(&node),
            now,
            None,
            None,
            true,
            Some(codex_period(&node, 7.0 * 86400.0)),
        ));
    }
    let extras = data
        .get("additional_rate_limits")
        .or_else(|| limit.get("additional_rate_limits"))
        .and_then(Value::as_array);
    if let Some(extras) = extras {
        for item in extras {
            let nested = item.get("rate_limit").filter(|v| v.is_object()).unwrap_or(item);
            let win = nested
                .get("primary_window")
                .filter(|v| v.is_object())
                .unwrap_or(nested);
            let name = item
                .get("limit_name")
                .or_else(|| item.get("title"))
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("추가");
            rows.push(window(
                "codex",
                "weekly_scoped",
                name,
                used_field(win),
                reset_field(win),
                now,
                None,
                None,
                true,
                Some(codex_period(win, 7.0 * 86400.0)),
            ));
        }
    }
    if let Some(credits) = data.get("credits").and_then(Value::as_object) {
        if credits.get("unlimited") != Some(&json!(true)) {
            let bal = as_f64(credits.get("balance")).unwrap_or(0.0);
            if credits.get("has_credits") == Some(&json!(true)) || bal > 0.0 {
                rows.push(window(
                    "codex",
                    "credits",
                    "크레딧",
                    None,
                    None,
                    now,
                    Some(bal),
                    None,
                    false,
                    None,
                ));
            }
        }
    }
    rows
}

fn parse_grok(data: &Value, now: f64) -> Vec<Value> {
    let cfg = data.get("config").filter(|v| v.is_object()).unwrap_or(data);
    let mut used = as_f64(cfg.get("creditUsagePercent"));
    let mut percent = used.is_some();
    let mut reset = None;
    let mut start = None;
    let mut period = json!({});
    if let Some(cur) = cfg.get("currentPeriod").and_then(Value::as_object) {
        period = Value::Object(cur.clone());
        reset = cur.get("end");
        start = cur.get("start");
    }
    reset = reset.or_else(|| cfg.get("billingPeriodEnd"));
    start = start.or_else(|| cfg.get("billingPeriodStart"));
    if used.is_none() {
        let mut limit = cents_val(data.get("monthlyLimit")).or_else(|| cents_val(cfg.get("monthlyLimit")));
        let mut spent = data
            .get("usage")
            .and_then(|u| cents_val(u.get("totalUsed")))
            .or_else(|| cents_val(data.get("onDemandUsed")));
        if spent.is_none() {
            spent = cents_val(cfg.get("onDemandUsed"));
            limit = limit.or_else(|| cents_val(cfg.get("onDemandCap")));
        } else {
            limit = limit.or_else(|| cents_val(data.get("onDemandCap")));
        }
        if let (Some(limit), Some(spent)) = (limit, spent) {
            if limit > 0.0 {
                used = Some(spent / limit);
                percent = false;
            }
        }
    }
    if let Some(cycle) = data.get("billingCycle").and_then(Value::as_object) {
        reset = reset.or_else(|| cycle.get("billingPeriodEnd"));
        start = start.or_else(|| cycle.get("billingPeriodStart"));
        if period.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            period = Value::Object(cycle.clone());
        }
    }
    let used = match used {
        Some(v) => v,
        None => return Vec::new(),
    };
    let ptype = period
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_uppercase();
    let mut label = if ptype.contains("WEEKLY") {
        "주간"
    } else if ptype.contains("MONTHLY") {
        "월간"
    } else {
        "크레딧"
    };
    let reset_ts = ts(reset, now);
    let start_ts = ts(start, now);
    if label == "크레딧" {
        if let Some(when) = reset_ts {
            let days = when - now;
            if (5.5 * 86400.0..9.0 * 86400.0).contains(&days) {
                label = "주간";
            } else if days >= 9.0 * 86400.0 {
                label = "월간";
            }
        }
    }
    let mut span = match (reset_ts, start_ts) {
        (Some(a), Some(b)) if a - b > 0.0 => Some(a - b),
        _ => None,
    };
    if span.is_none() {
        span = Some(if label == "주간" {
            7.0 * 86400.0
        } else if label == "월간" {
            30.0 * 86400.0
        } else {
            0.0
        })
        .filter(|s| *s > 0.0);
    }
    vec![window(
        "grok",
        "credits",
        label,
        Some(if percent { used } else { used * 100.0 }),
        reset,
        now,
        None,
        None,
        true,
        span,
    )]
}

fn codex_label(node: &Value, fallback: &str) -> String {
    let span = as_f64(node.get("limit_window_seconds")).unwrap_or(0.0) as i64;
    if span <= 0 {
        return fallback.into();
    }
    if span <= 6 * 3600 {
        "5h".into()
    } else if span <= 2 * 86400 {
        "일간".into()
    } else if span <= 10 * 86400 {
        "주간".into()
    } else {
        "월간".into()
    }
}

fn codex_period(node: &Value, fallback: f64) -> f64 {
    as_f64(node.get("limit_window_seconds"))
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(fallback)
}

const SHORT: &[(&str, &str)] = &[
    ("claude-code", "CC"),
    ("codex", "CDX"),
    ("grok", "GRK"),
];
const TITLES: &[(&str, &str)] = &[
    ("claude-code", "Claude Code"),
    ("codex", "Codex"),
    ("grok", "Grok"),
];

pub fn window_key(row: &Value) -> String {
    ["source", "kind", "label"]
        .iter()
        .map(|k| row.get(*k).and_then(Value::as_str).unwrap_or(""))
        .collect::<Vec<_>>()
        .join(":")
}

pub fn can_represent(row: &Value) -> bool {
    let source = row.get("source").and_then(Value::as_str).unwrap_or("");
    if source.is_empty() || row.get("status").and_then(Value::as_str) == Some("unavailable") {
        return false;
    }
    row.get("used").and_then(Value::as_f64).is_some()
        || row.get("remaining_usd").and_then(Value::as_f64).is_some()
}

pub fn representative_windows(windows: &[Value], prefs: &serde_json::Map<String, Value>) -> Vec<Value> {
    let mut grouped: BTreeMap<String, Vec<Value>> = BTreeMap::new();
    for row in windows {
        if can_represent(row) {
            let source = row
                .get("source")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            grouped.entry(source).or_default().push(row.clone());
        }
    }
    let mut picked = Vec::new();
    for (source, rows) in grouped {
        let preference = prefs
            .get(&source)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let row = rows
            .iter()
            .find(|item| window_key(item) == preference)
            .cloned()
            .or_else(|| {
                if preference.is_empty() {
                    None
                } else {
                    rows.iter()
                        .find(|item| item.get("kind").and_then(Value::as_str) == Some(&preference))
                        .cloned()
                }
            })
            .or_else(|| {
                if source == "claude-code" {
                    rows.iter()
                        .find(|item| item.get("kind").and_then(Value::as_str) == Some("weekly"))
                        .cloned()
                } else {
                    None
                }
            })
            .unwrap_or_else(|| rows[0].clone());
        picked.push(row);
    }
    picked
}

pub fn chips(windows: &[Value], prefs: &serde_json::Map<String, Value>) -> Vec<(String, String)> {
    let picked: BTreeMap<String, Value> = representative_windows(windows, prefs)
        .into_iter()
        .filter_map(|row| {
            row.get("source")
                .and_then(Value::as_str)
                .map(|s| (s.to_string(), row.clone()))
        })
        .collect();
    let mut out = Vec::new();
    for src in ["claude-code", "codex", "grok"] {
        let Some(row) = picked.get(src) else { continue };
        let tag = SHORT
            .iter()
            .find(|(k, _)| *k == src)
            .map(|(_, v)| *v)
            .unwrap_or(src);
        let text = if let Some(used) = row.get("used").and_then(Value::as_f64) {
            format!(
                "{tag} {} · {:.0}% 사용",
                row.get("label").and_then(Value::as_str).unwrap_or("?"),
                used * 100.0
            )
        } else if let Some(remain) = row.get("remaining_usd").and_then(Value::as_f64) {
            format!("{tag} ${remain:.1}")
        } else {
            continue;
        };
        let status = row.get("status").and_then(Value::as_str).unwrap_or("ok");
        let status = if status == "ok" && is_underused(row, None) {
            "underused"
        } else {
            status
        };
        out.push((text, status.into()));
    }
    out
}

pub fn pace_gap(row: &Value, now: Option<f64>) -> Option<f64> {
    let status = row
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    if matches!(status.as_str(), "stale" | "unavailable" | "expired") {
        return None;
    }
    let plan = row.get("plan").and_then(Value::as_str).unwrap_or("");
    if !plan.is_empty() && plan != "subscription" {
        return None;
    }
    let used = row.get("used").and_then(Value::as_f64)?;
    let period = row.get("period_seconds").and_then(Value::as_f64)?;
    let now = now.unwrap_or_else(now_secs);
    let reset = as_f64(row.get("resets_at")).or_else(|| ts(row.get("resets_at"), now))?;
    if !used.is_finite() || !period.is_finite() || period <= 0.0 || !reset.is_finite() || reset <= now
    {
        return None;
    }
    let elapsed = (1.0 - (reset - now) / period).clamp(0.0, 1.0);
    Some(elapsed - used)
}

pub fn is_underused(row: &Value, now: Option<f64>) -> bool {
    pace_gap(row, now).is_some_and(|g| g > 0.0)
}

pub fn panel_rows(windows: &[Value], now: Option<f64>) -> Vec<(String, String, f64, String, String)> {
    let now = now.unwrap_or_else(now_secs);
    windows
        .iter()
        .map(|row| {
            let used = row.get("used").and_then(Value::as_f64).unwrap_or(-1.0);
            let reset = reset_caption(as_f64(row.get("resets_at")), now);
            let label = if row.get("remaining_usd").is_some()
                && row.get("kind").and_then(Value::as_str) == Some("credits")
            {
                let remain = row.get("remaining_usd").and_then(Value::as_f64).unwrap_or(0.0);
                match row.get("cap_usd").and_then(Value::as_f64) {
                    Some(cap) => format!("${remain:.2} / ${cap:.0}"),
                    None => format!("${remain:.2}"),
                }
            } else {
                row.get("label")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let source = row.get("source").and_then(Value::as_str).unwrap_or("");
            let title = row
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    TITLES
                        .iter()
                        .find(|(k, _)| *k == source)
                        .map(|(_, v)| (*v).to_string())
                        .unwrap_or_else(|| source.to_string())
                });
            let status = row
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("ok")
                .to_string();
            (title, label, used, reset, status)
        })
        .collect()
}

pub fn reset_caption(ts: Option<f64>, now: f64) -> String {
    let when = match ts {
        Some(v) if v > 0.0 => v,
        _ => return String::new(),
    };
    let sec = (when - now) as i64;
    if sec <= 0 {
        return "곧".into();
    }
    if sec < 60 {
        format!("{sec}초")
    } else if sec < 3600 {
        format!("{}분", sec / 60)
    } else if sec < 86400 {
        format!("{}시간", sec / 3600)
    } else {
        format!("{}일", sec / 86400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_session_and_weekly() {
        let data = json!({
            "limits": [
                {"kind": "session", "percent": 10, "resets_at": 1.8e9},
                {"kind": "weekly_all", "percent": 40, "resets_at": 1.8e9}
            ]
        });
        let rows = parse_claude(&data, 1.7e9);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["kind"], "session");
        assert!((rows[0]["used"].as_f64().unwrap() - 0.10).abs() < 1e-9);
    }

    fn labels(rows: &[Value]) -> Vec<&str> {
        rows.iter().map(|r| r["label"].as_str().unwrap_or("")).collect()
    }

    fn prefs(source: &str, pick: &str) -> serde_json::Map<String, Value> {
        [(source.to_string(), json!(pick))].into_iter().collect()
    }

    #[test]
    fn provider_payloads_normalize_to_windows_chips_and_pace() {
        let now = 1_800_000_000.0;
        let claude = parse_claude(&json!({
            "five_hour": {"utilization": 38.0, "resets_at": "2027-01-01T12:00:00Z"},
            "seven_day": {"utilization": 15.0, "resets_at": "2027-01-07T00:00:00Z"},
            "seven_day_sonnet": {"utilization": 91.0, "resets_at": "2027-01-03T00:00:00Z"},
            "extra_usage": {"is_enabled": true, "monthly_limit": 100000, "used_credits": 2500}
        }), now);
        let kind = |k: &str| claude.iter().find(|r| r["kind"] == k).unwrap().clone();
        assert_eq!((kind("session")["used"].clone(), kind("session")["label"].clone()), (json!(0.38), json!("5h")));
        assert_eq!(kind("session")["period_seconds"], 5.0 * 3600.0);
        assert_eq!(parse_claude(&json!({"seven_day": {"utilization": 1.0}}), now)[0]["used"], 0.01);
        assert_eq!((kind("weekly")["used"].clone(), kind("weekly")["period_seconds"].clone()), (json!(0.15), json!(7.0 * 86400.0)));
        assert_eq!((kind("weekly_scoped")["label"].clone(), kind("weekly_scoped")["status"].clone()), (json!("Sonnet 주"), json!("exhausted")));
        assert_eq!((kind("credits")["remaining_usd"].clone(), kind("credits")["cap_usd"].clone()), (json!(975.0), json!(1000.0)));

        let structured = parse_claude(&json!({"limits": [
            {"kind": "session", "percent": 10, "resets_at": "2027-01-01T00:00:00Z"},
            {"kind": "weekly_all", "percent": 20, "resets_at": "2027-01-08T00:00:00Z"},
            {"kind": "weekly_scoped", "percent": 30, "resets_at": "2027-01-04T00:00:00Z",
             "scope": {"model": {"display_name": "Fable"}}}
        ]}), now);
        assert_eq!(labels(&structured), ["5h", "주간", "Fable 주"]);

        let codex = parse_codex(&json!({
            "plan_type": "plus",
            "rate_limit": {
                "primary_window": {"used_percent": 72, "reset_after_seconds": 3600},
                "secondary_window": {"used_percent": 48, "reset_at": now + 4.0 * 86400.0}
            },
            "credits": {"balance": 12.4, "unlimited": false}
        }), now);
        assert_eq!(labels(&codex), ["5h", "주간", "크레딧"]);
        assert_eq!((codex[0]["used"].clone(), codex[0]["status"].clone()), (json!(0.72), json!("warn")));
        assert_eq!((codex[0]["period_seconds"].clone(), codex[1]["period_seconds"].clone()), (json!(5.0 * 3600.0), json!(7.0 * 86400.0)));
        assert_eq!(codex[2]["remaining_usd"], 12.4);
        let weekly = parse_codex(&json!({
            "rate_limit": {"primary_window": {"used_percent": 1, "limit_window_seconds": 604800, "reset_after_seconds": 100}},
            "credits": {"balance": 0, "has_credits": false}
        }), now);
        assert_eq!((weekly[0]["label"].clone(), weekly[0]["used"].clone(), weekly[0]["period_seconds"].clone()), (json!("주간"), json!(0.01), json!(604800.0)));
        assert!(weekly.iter().all(|r| r["kind"] != "credits"));

        let grok = parse_grok(&json!({"config": {"creditUsagePercent": 22.0, "currentPeriod": {"end": "2027-02-01T00:00:00Z"}}}), now);
        assert_eq!((grok[0]["source"].clone(), grok[0]["used"].clone()), (json!("grok"), json!(0.22)));
        let ratio = parse_grok(&json!({"monthlyLimit": {"val": 20000}, "usage": {"totalUsed": {"val": 5000}},
                                       "billingCycle": {"billingPeriodEnd": "2027-02-01T00:00:00Z"}}), now);
        assert_eq!(ratio[0]["used"], 0.25);
        let monthly = parse_grok(&json!({"config": {
            "currentPeriod": {"type": "MONTHLY", "start": now - 86400.0, "end": now + 29.0 * 86400.0},
            "onDemandCap": {"val": 20000}, "onDemandUsed": {"val": 5000}
        }}), now);
        assert_eq!((monthly[0]["used"].clone(), monthly[0]["label"].clone()), (json!(0.25), json!("월간")));
        let exact = parse_grok(&json!({"config": {"creditUsagePercent": 10, "currentPeriod": {
            "type": "WEEKLY", "start": now - 86400.0, "end": now + 6.0 * 86400.0}}}), now);
        assert_eq!((exact[0]["label"].clone(), exact[0]["period_seconds"].clone()), (json!("주간"), json!(7.0 * 86400.0)));

        assert_eq!(reset_caption(Some(now + 90.0), now), "1분");
        assert_eq!(reset_caption(Some(now + 3.0 * 3600.0), now), "3시간");
        assert_eq!(reset_caption(Some(now + 6.0 * 86400.0), now), "6일");
        let all: Vec<Value> = claude.iter().chain(&codex).chain(&grok).cloned().collect();
        let texts: Vec<String> = chips(&all, &Default::default()).into_iter().map(|(t, _)| t).collect();
        assert!(texts[0].starts_with("CC ") && texts[0].contains("주간"), "{texts:?}");
        assert!(texts.iter().any(|t| t.starts_with("CDX ")) && texts.iter().any(|t| t.starts_with("GRK ")));

        let scoped = kind("weekly_scoped");
        let exact_key = window_key(&scoped);
        assert_eq!(exact_key, "claude-code:weekly_scoped:Sonnet 주");
        assert_eq!(representative_windows(&claude, &prefs("claude-code", &exact_key))[0], scoped);
        assert_eq!(representative_windows(&claude, &prefs("claude-code", "weekly_scoped"))[0], scoped);
        assert_eq!(representative_windows(&claude, &prefs("claude-code", "missing"))[0], kind("weekly"));
        assert!(chips(&claude, &prefs("claude-code", &exact_key))[0].0.contains("Sonnet 주"));

        let mut pace = kind("weekly");
        pace["used"] = json!(0.20);
        pace["resets_at"] = json!(now + 3.0 * 86400.0);
        assert!((pace_gap(&pace, Some(now)).unwrap() - (4.0 / 7.0 - 0.20)).abs() < 1e-12);
        assert!(is_underused(&pace, Some(now)));
        let mut live = pace.clone();
        live["resets_at"] = json!(now_secs() + 3.0 * 86400.0);
        assert_eq!(chips(&[live], &Default::default())[0].1, "underused");
        let mut even = pace.clone();
        even["used"] = json!(4.0 / 7.0);
        assert!(!is_underused(&even, Some(now)));
        for (field, value) in [("status", json!("stale")), ("period_seconds", Value::Null), ("resets_at", json!(now))] {
            let mut invalid = pace.clone();
            invalid[field] = value;
            assert!(pace_gap(&invalid, Some(now)).is_none() && !is_underused(&invalid, Some(now)), "{field}");
        }
    }
}
