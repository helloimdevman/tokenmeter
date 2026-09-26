//! Python `Meter`와 같은 state.json 계약을 지키는 단일 writer.

use crate::pricing::{cache_savings, cost_usd};
use crate::watch::TokenDelta;
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tokenmeter_hook::{data_dir, live_dir, live_path, project_key};
use tokenmeter_protocol::Label;

const DAYS_KEPT: usize = 60;
const HOURS_KEPT: usize = 24 * DAYS_KEPT;
const RATES_KEPT: usize = 4 * 24 * DAYS_KEPT;

pub struct Meter {
    pub state: Value,
    session_history: usize,
    state_path: Option<PathBuf>,
    clock: Clock,
}

impl Default for Meter {
    fn default() -> Self {
        Self::new()
    }
}

impl Meter {
    pub fn new() -> Self {
        let clock = current_clock();
        Self {
            state: default_state(&clock),
            session_history: 500,
            state_path: None,
            clock,
        }
    }

    pub fn load() -> Self {
        Self::load_from(state_file())
    }

    fn load_from(path: PathBuf) -> Self {
        let clock = current_clock();
        let mut state = default_state(&clock);
        if let Some(Value::Object(mut saved)) = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        {
            saved.remove("live");
            let mut saved = Value::Object(saved);
            migrate_legacy(&mut saved);
            deep_merge(&mut state, saved);
        }
        if let Some(obj) = state.as_object_mut() {
            obj.insert("version".into(), json!(2));
            obj.insert(
                "session".into(),
                json!({
                    "started_at": crate::watch::now_secs(), "totals": totals()
                }),
            );
        }
        Self {
            state,
            session_history: 500,
            state_path: Some(path),
            clock,
        }
    }

    #[cfg(test)]
    pub(crate) fn load_test(path: PathBuf) -> Self {
        Self::load_from(path)
    }

    pub fn set_session_history(&mut self, cap: usize) {
        self.session_history = cap.max(20);
    }

    pub fn ingest(&mut self, mut delta: TokenDelta) {
        self.resolve_project(&mut delta);
        let now = crate::watch::now_secs();
        self.refresh_clock(now);
        let local = delta.plan == "local";
        let cost = match delta.cost_usd {
            _ if local => 0.0,
            Some(logged) if logged.is_finite() && logged > 0.0 => logged,
            _ => cost_usd(
                &delta.model,
                delta.input_tokens,
                delta.cache_read,
                delta.cache_write,
                delta.output_tokens,
            ),
        };
        let saved = if local { 0.0 } else { cache_savings(&delta.model, delta.cache_read) };
        self.roll_today();
        self.roll_hour();
        self.roll_rate();
        if delta.speed_only {
            // 토큰은 다른 줄에서 이미 셌다. 기록된 API 시간 표본만 경로 셀에(스펙 10절).
            let timed = (delta.duration_ms > 0).then(|| delta.duration_ms as f64 / 1000.0);
            self.bucket_route(&delta, 0.0, timed);
            let _ = self.save();
            return;
        }
        self.bucket_hour(&delta, cost);

        for path in ["/total/totals", "/session/totals", "/today/totals"] {
            if let Some(book) = self.state.pointer_mut(path).and_then(Value::as_object_mut) {
                accumulate(book, &delta, cost, saved);
            }
        }

        let axes = [
            ("projects", nonempty(&delta.project, "(unknown)")),
            ("services", nonempty(&delta.service, "(unknown)")),
            ("models", nonempty(&delta.model, "default")),
            ("vendors", nonempty(&delta.vendor, "unknown")),
            ("plans", nonempty(&delta.plan, "unknown")),
            ("endpoints", nonempty(&delta.endpoint, "unknown")),
        ];
        for (group, name) in &axes {
            let node = group_node(&mut self.state, group, name);
            let book = node
                .entry("totals")
                .or_insert_with(|| Value::Object(totals()))
                .as_object_mut()
                .expect("totals object");
            accumulate(book, &delta, cost, saved);
            node.insert("last_seen".into(), json!(now));
            if *group == "models" && !delta.vendor.is_empty() {
                node.insert("vendor".into(), json!(delta.vendor));
            }
        }

        let timed = self.track_session(&delta, cost, saved, now, &axes);
        self.bucket_route(&delta, cost, timed);
        if let Some(total) = self.state.get_mut("total").and_then(Value::as_object_mut) {
            total.insert("last_seen".into(), json!(now));
        }
        touch_live(&delta);
        let _ = self.save();
    }

    fn track_session(
        &mut self,
        delta: &TokenDelta,
        cost: f64,
        saved: f64,
        now: f64,
        axes: &[(&str, String)],
    ) -> Option<f64> {
        if delta.session.is_empty() {
            return None;
        }
        let key = format!("{}/{}", nonempty(&delta.service, "?"), delta.session);
        let is_new = self
            .state
            .get("sessions")
            .and_then(Value::as_object)
            .map(|book| !book.contains_key(&key))
            .unwrap_or(true);
        if is_new {
            if let Some(total) = self.state.get_mut("total").and_then(Value::as_object_mut) {
                let count = int(total.get("sessions")) + 1;
                total.insert("sessions".into(), json!(count));
            }
        }

        let mut new_tags = Vec::new();
        let mut rate: Option<(String, f64)> = None;
        {
            let sessions = self
                .state
                .as_object_mut()
                .expect("state object")
                .entry("sessions")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("sessions object");
            let rec = sessions.entry(&key).or_insert_with(|| {
                json!({
                    "service": delta.service, "project": delta.project, "cwd": delta.cwd,
                    "vendor": delta.vendor, "plan": nonempty(&delta.plan, "unknown"),
                    "endpoint": delta.endpoint, "started_at": now, "seen": [], "totals": totals(),
                })
            });
            let rec = rec.as_object_mut().expect("session object");
            if !delta.subagent {
                rec.insert("model".into(), json!(delta.model));
                if !delta.cwd.is_empty() {
                    rec.insert("cwd".into(), json!(delta.cwd));
                    rec.insert("project".into(), json!(delta.project));
                }
                for (field, value) in [
                    ("vendor", &delta.vendor),
                    ("plan", &delta.plan),
                    ("endpoint", &delta.endpoint),
                    ("effort", &delta.effort),
                ] {
                    if !value.is_empty() {
                        rec.insert(field.into(), json!(value));
                    }
                }
            } else {
                add_number(rec, "sub_cost", cost);
                add_int(rec, "sub_output_tokens", delta.output_tokens);
            }
            if delta.ctx_tokens > 0 {
                rec.insert("ctx".into(), json!(delta.ctx_tokens));
                rec.insert(
                    "ctx_win".into(),
                    json!(int(rec.get("ctx_win")).max(delta.ctx_window)),
                );
            }
            if delta.output_tokens > 0 && !delta.subagent {
                let stream = format!(
                    "{}/{}",
                    provider_of(&delta.endpoint, &delta.vendor),
                    delta.model
                );
                let previous = if rec.get("out_model").and_then(Value::as_str) == Some(&stream) {
                    number(rec.get("out_at"))
                } else {
                    0.0
                };
                let gap = if delta.duration_ms > 0 {
                    delta.duration_ms as f64 / 1000.0
                } else if previous > 0.0 && now > previous && now - previous <= 30.0 {
                    now - previous
                } else {
                    0.0
                };
                rec.insert("out_at".into(), json!(now));
                rec.insert("out_model".into(), json!(stream.clone()));
                rate = (gap > 0.0).then_some((stream, gap));
            }
            rec.insert("last_seen".into(), json!(now));
            let book = rec
                .entry("totals")
                .or_insert_with(|| Value::Object(totals()))
                .as_object_mut()
                .expect("session totals");
            accumulate(book, delta, cost, saved);
            let seen: HashSet<String> = rec
                .get("seen")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            for (group, name) in axes {
                let tag = format!("{group}:{name}");
                if !seen.contains(&tag) {
                    new_tags.push(tag);
                }
            }
            rec.entry("seen")
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .expect("seen array")
                .extend(new_tags.iter().map(|tag| json!(tag)));
        }

        for tag in &new_tags {
            let Some((group, name)) = tag.split_once(':') else {
                continue;
            };
            add_int(group_node(&mut self.state, group, name), "sessions", 1);
        }
        let timed = rate.as_ref().map(|(_, gap)| *gap);
        if let Some((stream, gap)) = rate {
            self.state.as_object_mut().expect("state object").insert(
                "out_sec".into(),
                json!(if delta.duration_ms > 0 { gap } else { 0.0 }),
            );
            let book = self
                .state
                .pointer_mut("/rate/m")
                .and_then(Value::as_object_mut)
                .expect("rate book");
            let cell = book
                .entry(stream)
                .or_insert_with(|| json!([0, 0.0, 0]))
                .as_array_mut()
                .expect("rate cell");
            cell[0] = json!(int(cell.first()) + delta.output_tokens);
            cell[1] = json!(number(cell.get(1)) + gap);
            cell[2] = json!(int(cell.get(2)) + 1);
        }
        self.trim_sessions();
        timed
    }

    fn trim_sessions(&mut self) {
        let Some(book) = self
            .state
            .get_mut("sessions")
            .and_then(Value::as_object_mut)
        else {
            return;
        };
        if book.len() <= self.session_history {
            return;
        }
        let mut order: Vec<(String, f64)> = book
            .iter()
            .map(|(key, rec)| (key.clone(), number(rec.get("last_seen"))))
            .collect();
        order.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        for (key, _) in order.into_iter().take(book.len() - self.session_history) {
            book.remove(&key);
        }
    }

    fn resolve_project(&self, delta: &mut TokenDelta) {
        if !delta.project.is_empty() || delta.session.is_empty() {
            return;
        }
        let Ok(text) = fs::read_to_string(live_path(&delta.service, &delta.session)) else {
            return;
        };
        let Ok(rec) = serde_json::from_str::<Value>(&text) else {
            return;
        };
        let cwd = rec.get("cwd").and_then(Value::as_str).unwrap_or_default();
        if delta.cwd.is_empty() {
            delta.cwd = cwd.to_string();
        }
        delta.project = {
            let project = project_key(cwd);
            if project.is_empty() {
                rec.get("project")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            } else {
                project
            }
        };
    }

    fn roll_today(&mut self) {
        let today = self.clock.day.clone();
        if today.is_empty() {
            return;
        }
        let previous = self
            .state
            .pointer("/today/date")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if previous == today {
            return;
        }
        let old = self
            .state
            .get("today")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let old_day = old.get("date").and_then(Value::as_str).unwrap_or_default();
        let old_totals = old
            .get("totals")
            .cloned()
            .unwrap_or_else(|| Value::Object(totals()));
        if !old_day.is_empty() && tokens_of(&old_totals) > 0 {
            let days = self
                .state
                .as_object_mut()
                .expect("state object")
                .entry("days")
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .expect("days object");
            days.insert(old_day.to_string(), old_totals);
            let excess = days.len().saturating_sub(DAYS_KEPT);
            let mut keys: Vec<String> = days.keys().cloned().collect();
            keys.sort();
            for key in keys.into_iter().take(excess) {
                days.remove(&key);
            }
        }
        self.state
            .as_object_mut()
            .expect("state object")
            .insert("today".into(), json!({"date": today, "totals": totals()}));
    }

    fn roll_hour(&mut self) {
        let hour = self.clock.hour.clone();
        if hour.is_empty() || self.state.pointer("/hour/h").and_then(Value::as_str) == Some(&hour) {
            return;
        }
        let old = self.state.get("hour").cloned().unwrap_or_else(|| json!({}));
        if old
            .get("h")
            .and_then(Value::as_str)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
            && old
                .get("p")
                .and_then(Value::as_object)
                .map(|v| !v.is_empty())
                .unwrap_or(false)
        {
            append_limited(&data_dir().join("hours.jsonl"), &old, HOURS_KEPT);
        }
        self.state
            .as_object_mut()
            .expect("state object")
            .insert("hour".into(), json!({"h": hour, "p": {}, "r": {}}));
    }

    fn bucket_hour(&mut self, delta: &TokenDelta, cost: f64) {
        let name = nonempty(&delta.project, "(unknown)");
        let cell = self
            .state
            .pointer_mut("/hour/p")
            .and_then(Value::as_object_mut)
            .expect("hour book")
            .entry(name)
            .or_insert_with(|| json!([0, 0.0, 0]))
            .as_array_mut()
            .expect("hour cell");
        cell[0] = json!(int(cell.first()) + delta.total());
        cell[1] = json!(number(cell.get(1)) + cost);
        cell[2] = json!(int(cell.get(2)) + i64::from(delta.calls));
    }

    /// 리그 동기화용 시간 칸: (도구, 경로, 요금제, 모델)마다 한 셀. 라벨은 모두 올려도 되는 값이다.
    fn bucket_route(&mut self, delta: &TokenDelta, cost: f64, timed: Option<f64>) {
        let hour = self
            .state
            .get_mut("hour")
            .and_then(Value::as_object_mut)
            .expect("hour book");
        let book = hour
            .entry("r")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("route book");
        let cell = book
            .entry(route_key(delta))
            .or_insert_with(|| json!([0, 0, 0, 0, 0, 0.0, 0, 0, 0, 0]))
            .as_array_mut()
            .expect("route cell");
        if !delta.speed_only {
            let counts = [
                delta.input_tokens,
                delta.output_tokens,
                delta.cache_read,
                delta.cache_write,
                i64::from(delta.calls),
            ];
            for (i, n) in counts.into_iter().enumerate() {
                cell[i] = json!(int(cell.get(i)) + n);
            }
            cell[5] = json!(number(cell.get(5)) + cost);
        }
        if let Some(secs) = timed {
            let at = if delta.duration_ms > 0 { 6 } else { 8 };
            cell[at] = json!(int(cell.get(at)) + delta.output_tokens);
            cell[at + 1] = json!(int(cell.get(at + 1)) + (secs * 1000.0).round() as i64);
        }
    }

    fn roll_rate(&mut self) {
        let slot = self.clock.slot.clone();
        if slot.is_empty() || self.state.pointer("/rate/h").and_then(Value::as_str) == Some(&slot) {
            return;
        }
        let old = self.state.get("rate").cloned().unwrap_or_else(|| json!({}));
        if old
            .get("h")
            .and_then(Value::as_str)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
            && old
                .get("m")
                .and_then(Value::as_object)
                .map(|v| !v.is_empty())
                .unwrap_or(false)
        {
            append_limited(&data_dir().join("rates.jsonl"), &old, RATES_KEPT);
        }
        self.state
            .as_object_mut()
            .expect("state object")
            .insert("rate".into(), json!({"h": slot, "m": {}}));
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let Some(path) = self.state_path.clone() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.state
            .as_object_mut()
            .expect("state object")
            .insert("updated_at".into(), json!(crate::watch::now_secs()));
        let tmp = path.with_file_name(format!("state.json.{}.tmp", std::process::id()));
        fs::write(&tmp, serde_json::to_string_pretty(&self.state)?)?;
        match fs::rename(&tmp, &path) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = fs::remove_file(&tmp);
                Err(err)
            }
        }
    }

    fn refresh_clock(&mut self, now: f64) {
        let minute = (now / 60.0) as i64;
        if minute != self.clock.minute {
            self.clock = current_clock();
        }
    }

    pub fn refresh_live(&mut self) {
        self.state
            .as_object_mut()
            .expect("state object")
            .insert("live".into(), Value::Array(read_live()));
    }

    pub fn status(&self) -> Value {
        let mut out = self.state.clone();
        let live = read_live();
        let count = live.len();
        if let Some(obj) = out.as_object_mut() {
            obj.insert("live".into(), Value::Array(live));
            obj.insert("live_count".into(), json!(count));
        }
        out
    }

    pub fn reset_stats(&mut self) {
        let clock = current_clock();
        let now = crate::watch::now_secs();
        self.state = default_state(&clock);
        if let Some(obj) = self.state.as_object_mut() {
            obj.insert("updated_at".into(), json!(now));
        }
        let _ = fs::remove_file(data_dir().join("hours.jsonl"));
        let _ = fs::remove_file(data_dir().join("rates.jsonl"));
        let _ = self.save();
    }

    pub fn add_live(
        &self,
        service: &str,
        session_id: &str,
        project: &str,
        cwd: &str,
        model: &str,
        event: &str,
    ) -> PathBuf {
        let sid = if session_id.is_empty() {
            format!("{}-{}", service, crate::watch::now_secs() as i64)
        } else {
            session_id.to_string()
        };
        let path = live_path(service, &sid);
        let rec = json!({
            "service": service,
            "session_id": sid,
            "project": if project.is_empty() { project_key(cwd) } else { project.to_string() },
            "cwd": cwd,
            "model": model,
            "started_at": crate::watch::now_secs(),
            "event": event,
        });
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, rec.to_string());
        path
    }

    pub fn remove_live(&self, service: &str, session_id: &str) -> bool {
        fs::remove_file(live_path(service, session_id)).is_ok()
    }

    pub fn archive(&self, service: &str, session_id: &str) {
        let path = live_path(service, session_id);
        let mut rec = fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or_else(|| json!({"service": service, "session_id": session_id}));
        let ended = crate::watch::now_secs();
        if let Some(obj) = rec.as_object_mut() {
            obj.insert("ended_at".into(), json!(ended));
            obj.insert(
                "session_totals".into(),
                self.state
                    .pointer("/session/totals")
                    .cloned()
                    .unwrap_or(Value::Object(totals())),
            );
            obj.insert(
                "today_totals".into(),
                self.state
                    .pointer("/today/totals")
                    .cloned()
                    .unwrap_or(Value::Object(totals())),
            );
            obj.insert(
                "total_totals".into(),
                self.state
                    .pointer("/total/totals")
                    .cloned()
                    .unwrap_or(Value::Object(totals())),
            );
        }
        let hist = data_dir().join("history");
        let _ = fs::create_dir_all(&hist);
        let name = format!(
            "{}__{}__{}.json",
            tokenmeter_hook::safe_name(service),
            tokenmeter_hook::safe_name(session_id),
            ended as i64
        );
        let _ = fs::write(hist.join(name), rec.to_string());
    }

    pub fn reload(&mut self) {
        if let Some(path) = self.state_path.clone() {
            *self = Self::load_from(path);
        }
    }
}

fn totals() -> Map<String, Value> {
    [
        ("input_tokens", json!(0)),
        ("cache_read", json!(0)),
        ("cache_write", json!(0)),
        ("output_tokens", json!(0)),
        ("cost_usd", json!(0.0)),
        ("cache_saved_usd", json!(0.0)),
        ("calls", json!(0)),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect()
}

fn default_state(clock: &Clock) -> Value {
    let now = crate::watch::now_secs();
    json!({
        "version": 2,
        "total": {"started_at": now, "last_seen": 0.0, "sessions": 0, "totals": totals()},
        "session": {"started_at": now, "totals": totals()},
        "today": {"date": clock.day, "totals": totals()},
        "days": {}, "hour": {"h": clock.hour, "p": {}, "r": {}},
        "rate": {"h": clock.slot, "m": {}}, "sessions": {},
        "projects": {}, "services": {}, "models": {}, "vendors": {}, "plans": {}, "endpoints": {},
        "updated_at": now,
    })
}

fn accumulate(book: &mut Map<String, Value>, delta: &TokenDelta, cost: f64, saved: f64) {
    add_int(book, "input_tokens", delta.input_tokens);
    add_int(book, "cache_read", delta.cache_read);
    add_int(book, "cache_write", delta.cache_write);
    add_int(book, "output_tokens", delta.output_tokens);
    add_number(book, "cost_usd", cost);
    add_number(book, "cache_saved_usd", saved);
    add_int(book, "calls", i64::from(delta.calls));
}

fn group_node<'a>(state: &'a mut Value, group: &str, name: &str) -> &'a mut Map<String, Value> {
    state
        .as_object_mut()
        .expect("state object")
        .entry(group)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("group object")
        .entry(name)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .expect("group row")
}

fn add_int(book: &mut Map<String, Value>, key: &str, value: i64) {
    book.insert(key.into(), json!(int(book.get(key)) + value));
}

fn add_number(book: &mut Map<String, Value>, key: &str, value: f64) {
    book.insert(key.into(), json!(number(book.get(key)) + value));
}

fn int(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_f64().map(|n| n as i64))
                .or_else(|| v.as_str()?.parse().ok())
        })
        .unwrap_or(0)
}

fn number(value: Option<&Value>) -> f64 {
    let n = value
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .unwrap_or(0.0);
    if n.is_finite() {
        n
    } else {
        0.0
    }
}

fn nonempty(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.into()
    } else {
        value.into()
    }
}

/// 경로 칸의 라벨(client, route, plan, model).
/// 사용자가 추가한 서비스, 사설 호스트, 사내 모델 이름은 고정된 말로 바뀐다.
pub fn route_labels(delta: &TokenDelta) -> [String; 4] {
    let client = if crate::watch::is_builtin_service(&delta.service) {
        delta.service.as_str()
    } else {
        "other"
    };
    // 요금제는 기본 서비스가 내는 값만 그대로 둔다. services.yaml에 사용자가 적은 계약명은 새지 않게 other로.
    let plan = match delta.plan.as_str() {
        "" => "unknown",
        p @ ("subscription" | "api" | "local" | "unknown") => p,
        _ => "other",
    };
    [
        Label::Client.clean(client),
        Label::Route.clean(&crate::board::public_label(&delta.endpoint)),
        plan.to_string(),
        crate::pricing::public_model(&delta.model).to_string(),
    ]
}

fn route_key(delta: &TokenDelta) -> String {
    route_labels(delta).join("\u{1f}")
}

fn tokens_of(value: &Value) -> i64 {
    ["input_tokens", "cache_read", "cache_write", "output_tokens"]
        .into_iter()
        .map(|key| int(value.get(key)))
        .sum()
}

fn provider_of(endpoint: &str, vendor: &str) -> String {
    let text = endpoint.trim();
    if text.is_empty() {
        return nonempty(vendor, "unknown");
    }
    if !text.contains("://") {
        return text.split('/').next().unwrap_or(text).to_string();
    }
    text.split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| nonempty(vendor, "unknown"))
}

fn state_file() -> PathBuf {
    data_dir().join("state.json")
}

fn migrate_legacy(state: &mut Value) {
    let Some(obj) = state.as_object_mut() else {
        return;
    };
    let Some(legacy) = obj.remove("pet") else {
        return;
    };
    let total = obj.entry("total").or_insert_with(|| json!({}));
    for (from, to) in [("born_at", "started_at"), ("last_fed", "last_seen")] {
        if let Some(at) = legacy.get(from).filter(|v| number(Some(v)) > 0.0) {
            total[to] = at.clone();
        }
    }
    let Some(old) = legacy.get("totals").cloned() else {
        return;
    };
    if total.get("totals").map(tokens_of).unwrap_or(0) == 0 {
        total["totals"] = old;
    }
}

fn deep_merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(base), Value::Object(over)) => {
            for (key, value) in over {
                if let Some(existing) = base.get_mut(&key) {
                    deep_merge(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, over) => *base = over,
    }
}

fn read_live() -> Vec<Value> {
    let mut live = Vec::new();
    if let Ok(entries) = fs::read_dir(live_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut record) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let Some(obj) = record.as_object_mut() else {
                continue;
            };
            let stem = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            let (service, session) = stem.split_once("__").unwrap_or((stem, ""));
            obj.entry("service").or_insert_with(|| json!(service));
            obj.entry("session_id").or_insert_with(|| json!(session));
            let modified = path
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs_f64())
                .unwrap_or(0.0);
            obj.insert("updated_at".into(), json!(modified));
            live.push(record);
        }
    }
    live
}

fn touch_live(delta: &TokenDelta) {
    if delta.session.is_empty() {
        return;
    }
    let path = live_path(&delta.service, &delta.session);
    if path.exists() {
        let _ = Command::new("touch").arg(path).status();
    }
}

fn local_format(format: &str) -> String {
    Command::new("date")
        .arg(format)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

#[derive(Clone, Default)]
struct Clock {
    minute: i64,
    day: String,
    hour: String,
    slot: String,
}

fn current_clock() -> Clock {
    let value = local_format("+%Y-%m-%dT%H:%M");
    let Some((prefix, minute)) = value.rsplit_once(':') else {
        return Clock::default();
    };
    let minute = minute.parse::<u32>().unwrap_or(0);
    Clock {
        minute: (crate::watch::now_secs() / 60.0) as i64,
        day: value.get(..10).unwrap_or_default().to_string(),
        hour: value.get(..13).unwrap_or_default().to_string(),
        slot: format!("{prefix}:{:02}", minute - minute % 15),
    }
}

fn append_limited(path: &Path, value: &Value, cap: usize) {
    let Ok(line) = serde_json::to_string(value) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(file, "{line}");
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() > cap {
        let _ = fs::write(path, format!("{}\n", lines[lines.len() - cap..].join("\n")));
    }
}

pub fn pid_file() -> PathBuf {
    data_dir().join("tokenmeter.pid")
}
pub fn lock_file() -> PathBuf {
    data_dir().join("daemon.lock")
}
pub fn log_file() -> PathBuf {
    data_dir().join("daemon.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(tokens: i64, project: &str) -> TokenDelta {
        TokenDelta {
            output_tokens: tokens,
            project: project.into(),
            model: "claude-opus-5".into(),
            ..TokenDelta::default()
        }
    }

    fn keys(value: &Value) -> Vec<String> {
        value.as_object().map(|o| o.keys().cloned().collect()).unwrap_or_default()
    }

    #[test]
    fn ingest_fills_every_bucket_persists_and_resets() {
        let (_g, tmp) = crate::test_home("buckets");
        let path = tmp.join("state.json");
        let mut meter = Meter::load_test(path.clone());
        meter.ingest(TokenDelta {
            input_tokens: 1000,
            cache_read: 1000,
            cache_write: 1000,
            output_tokens: 1000,
            model: "claude-opus-5".into(),
            service: "claude-code".into(),
            project: "tokenmeter".into(),
            ..TokenDelta::default()
        });
        assert!(number(meter.state.pointer("/total/totals/cost_usd")) > 0.0);
        meter.ingest(TokenDelta {
            input_tokens: 100_000,
            model: "claude-opus-5".into(),
            ..TokenDelta::default()
        });
        let state = meter.status();
        for bucket in ["total", "session", "today"] {
            assert_eq!(state[bucket]["totals"]["input_tokens"], 101_000, "{bucket}");
        }
        assert_eq!(state["projects"]["tokenmeter"]["totals"]["input_tokens"], 1000);
        assert_eq!(state["projects"]["(unknown)"]["totals"]["input_tokens"], 100_000);
        assert_eq!(state["services"]["claude-code"]["totals"]["output_tokens"], 1000);
        assert_eq!(state["models"]["claude-opus-5"]["totals"]["cache_read"], 1000);
        assert!(state.get("live_count").is_some());
        assert_eq!(
            Meter::load_test(path).state["total"]["totals"]["input_tokens"],
            101_000,
            "원자적으로 저장되고 다시 열면 그대로 읽힌다"
        );

        meter.reset_stats();
        assert_eq!(meter.state["total"]["totals"]["input_tokens"], 0);
        assert_eq!(meter.state["today"]["totals"]["cost_usd"], 0.0);
        assert_eq!((meter.state["projects"].clone(), meter.state["models"].clone()), (json!({}), json!({})));
    }

    #[test]
    fn calls_sessions_and_session_history_cap() {
        let (_g, tmp) = crate::test_home("calls");
        let mut meter = Meter::load_test(tmp.join("state.json"));
        let feed = |meter: &mut Meter, model: &str, vendor: &str, plan: &str, session: &str| {
            meter.ingest(TokenDelta {
                input_tokens: 100,
                model: model.into(),
                service: "claude-code".into(),
                project: "p".into(),
                session: session.into(),
                vendor: vendor.into(),
                plan: plan.into(),
                ..TokenDelta::default()
            })
        };
        feed(&mut meter, "claude-opus-5", "anthropic", "subscription", "s1");
        feed(&mut meter, "claude-opus-5", "anthropic", "subscription", "s1");
        feed(&mut meter, "claude-sonnet-5", "anthropic", "subscription", "s1");
        feed(&mut meter, "gpt-5.6-sol", "openai", "api", "s2");
        let st = &meter.state;
        assert_eq!(st["total"]["totals"]["calls"], 4, "호출 = 델타 건수");
        assert_eq!(st["models"]["claude-opus-5"]["totals"]["calls"], 2);
        assert_eq!(st["vendors"]["anthropic"]["totals"]["calls"], 3);
        assert_eq!(st["plans"]["api"]["totals"]["calls"], 1);
        assert_eq!(st["total"]["sessions"], 2);
        assert_eq!(st["models"]["claude-opus-5"]["sessions"], 1);
        assert_eq!(st["models"]["claude-sonnet-5"]["sessions"], 1, "모델을 바꾼 세션은 두 모델 모두 1세션");
        assert_eq!(st["vendors"]["anthropic"]["sessions"], 1, "같은 세션을 두 번 세면 안 된다");
        assert_eq!(st["vendors"]["openai"]["sessions"], 1);
        assert_eq!(st["services"]["claude-code"]["sessions"], 2);
        assert_eq!(st["models"]["gpt-5.6-sol"]["vendor"], "openai", "모델 노드는 벤더를 기억한다");

        meter.set_session_history(20);
        for i in 0..40 {
            feed(&mut meter, "claude-opus-5", "anthropic", "subscription", &format!("bulk-{i}"));
        }
        let sessions = meter.state["sessions"].as_object().unwrap();
        assert_eq!(sessions.len(), 20, "세션 기록은 상한을 넘으면 오래된 것부터 버린다");
        assert!(sessions.contains_key("claude-code/bulk-39") && !sessions.contains_key("claude-code/bulk-0"));

        let before = meter.state["total"]["sessions"].clone();
        meter.ingest(TokenDelta {
            input_tokens: 50,
            model: "x".into(),
            service: "s".into(),
            ..TokenDelta::default()
        });
        assert_eq!(meter.state["total"]["sessions"], before, "세션 id 없는 델타는 세션 집계에서만 빠진다");
        assert_eq!(meter.state["total"]["totals"]["input_tokens"], 100 * 44 + 50);
    }

    #[test]
    fn day_rollover_keeps_history_and_caps_it() {
        let (_g, _tmp) = crate::test_home("days");
        let mut meter = Meter::new();
        meter.ingest(out(100, ""));
        meter.state["today"]["date"] = json!("2026-08-10");
        meter.ingest(out(7, ""));
        assert_eq!(keys(&meter.state["days"]), ["2026-08-10"]);
        assert_eq!(meter.state["days"]["2026-08-10"]["output_tokens"], 100);
        assert_eq!(meter.state["today"]["totals"]["output_tokens"], 7, "새 날은 새 버킷");

        for i in 1..=70 {
            meter.state["days"][format!("2026-01-{i:02}")] = json!({"output_tokens": 1});
        }
        meter.state["today"]["date"] = json!("2020-01-01");
        meter.ingest(out(1, ""));
        assert_eq!(keys(&meter.state["days"]).len(), DAYS_KEPT);
        assert!(meter.state["days"].get("2026-01-01").is_none(), "오래된 날부터 버린다");

        meter.reset_stats();
        assert_eq!(meter.state["days"], json!({}));
    }

    #[test]
    fn hour_rollover_appends_hours_jsonl_and_reset_removes_it() {
        let (_g, _tmp) = crate::test_home("hours");
        let hours = data_dir().join("hours.jsonl");
        let mut meter = Meter::new();
        meter.ingest(out(100, "a"));
        assert_eq!(meter.state["hour"]["p"]["a"][0], 100);
        assert!(!hours.exists(), "진행 중인 시간은 아직 파일로 안 나간다");

        meter.state["hour"]["h"] = json!("2020-01-01T00");
        meter.ingest(out(7, "b"));
        let text = fs::read_to_string(&hours).unwrap();
        let done: Vec<Value> = text.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(done.len(), 1);
        assert_eq!(done[0]["h"], "2020-01-01T00");
        assert_eq!((done[0]["p"]["a"][0].clone(), done[0]["p"]["a"][2].clone()), (json!(100), json!(1)));
        assert_eq!(keys(&meter.state["hour"]["p"]), ["b"], "새 시간은 새 버킷");

        meter.state["hour"] = json!({"h": "2020-01-01T01", "p": {}});
        meter.ingest(out(1, "c"));
        assert_eq!(fs::read_to_string(&hours).unwrap().lines().count(), 1, "토큰 없는 시간은 남기지 않는다");

        meter.reset_stats();
        assert_eq!(meter.state["hour"]["p"], json!({}));
        assert!(!hours.exists(), "리셋은 시간 기록도 지운다");
    }

    #[test]
    fn subagent_output_counts_but_keeps_main_model_context_and_rate() {
        let (_g, _tmp) = crate::test_home("subagent");
        let mut meter = Meter::new();
        let base = TokenDelta {
            model: "claude-opus-5".into(),
            service: "claude-code".into(),
            session: "s".into(),
            vendor: "anthropic".into(),
            ..TokenDelta::default()
        };
        meter.ingest(TokenDelta {
            output_tokens: 100,
            effort: "high".into(),
            ctx_tokens: 1_000,
            ctx_window: 200_000,
            ..base.clone()
        });
        meter.ingest(TokenDelta {
            output_tokens: 300,
            effort: "low".into(),
            subagent: true,
            ..base.clone()
        });
        let rec = meter.state["sessions"]["claude-code/s"].clone();
        assert_eq!(rec["totals"]["output_tokens"], 400);
        assert_eq!(rec["sub_output_tokens"], 300);
        assert!((number(rec.get("sub_cost")) / number(rec["totals"].get("cost_usd")) - 0.75).abs() < 1e-9);
        assert_eq!(rec["ctx"], 1_000, "서브에이전트가 세션 컨텍스트를 건드리면 안 된다");
        assert_eq!(rec["effort"], "high");

        meter.ingest(TokenDelta {
            input_tokens: 1,
            model: "sub-model".into(),
            subagent: true,
            ..base
        });
        assert_eq!(meter.state["sessions"]["claude-code/s"]["model"], "claude-opus-5");
        assert!(keys(&meter.state["rate"]["m"]).iter().all(|k| !k.contains("sub-model")));
        let view = crate::attention::session_views(&meter.status(), crate::watch::now_secs())
            .into_iter()
            .find(|v| v.key == "claude-code/s")
            .unwrap();
        assert_eq!((view.main_output_tokens, view.output_tokens), (100, 400));
    }

    #[test]
    fn cache_savings_accumulate_next_to_cost() {
        let (_g, _tmp) = crate::test_home("cache");
        assert!((cache_savings("claude-opus-5", 1_000_000) - 4.5).abs() < 1e-9);
        assert_eq!(cache_savings("claude-opus-5", 0), 0.0);
        let mut meter = Meter::new();
        meter.ingest(TokenDelta {
            cache_read: 1_000_000,
            model: "claude-opus-5".into(),
            session: "s1".into(),
            ..TokenDelta::default()
        });
        for path in ["/total/totals", "/today/totals", "/session/totals", "/models/claude-opus-5/totals", "/sessions/?~1s1/totals"] {
            let saved = number(meter.state.pointer(&format!("{path}/cache_saved_usd")));
            assert!((saved - 4.5).abs() < 1e-9, "{path}");
        }
        assert!((number(meter.state.pointer("/total/totals/cost_usd")) - 0.5).abs() < 1e-9, "절감액은 비용이 아니다");
    }

    #[test]
    fn output_rate_uses_logged_duration_or_30s_bursts() {
        let (_g, _tmp) = crate::test_home("rate");
        let mut meter = Meter::new();
        meter.ingest(TokenDelta {
            output_tokens: 24704,
            session: "g1".into(),
            service: "grok".into(),
            vendor: "xai".into(),
            endpoint: "https://cli-chat-proxy.grok.com".into(),
            model: "grok-4.6-build".into(),
            duration_ms: 402_847,
            ..TokenDelta::default()
        });
        let cell = meter.state["rate"]["m"]["cli-chat-proxy.grok.com/grok-4.6-build"].clone();
        assert_eq!(cell[0], 24704);
        let secs = number(cell.get(1));
        assert!((400.0..=405.0).contains(&secs), "{secs}");
        assert!((24704.0 / secs - 61.32).abs() < 0.2);

        let stream = "api.deepseek.com/deepseek-v4-flash";
        let flash = |tokens: i64| TokenDelta {
            output_tokens: tokens,
            session: "s1".into(),
            service: "claude-code".into(),
            vendor: "anthropic".into(),
            endpoint: "https://api.deepseek.com/anthropic".into(),
            model: "deepseek-v4-flash".into(),
            project: "api".into(),
            ..TokenDelta::default()
        };
        meter.ingest(flash(80));
        assert!(meter.state["sessions"]["claude-code/s1"].get("out_at").is_some());
        assert!(meter.state["rate"]["m"].get(stream).is_none(), "첫 출력은 기준점일 뿐");

        meter.state["sessions"]["claude-code/s1"]["out_at"] = json!(crate::watch::now_secs() - 10.0);
        meter.ingest(flash(200));
        let cell = meter.state["rate"]["m"][stream].clone();
        assert_eq!(cell[0], 200);
        assert!((8.0..=12.0).contains(&number(cell.get(1))));

        meter.state["sessions"]["claude-code/s1"]["out_at"] = json!(crate::watch::now_secs() - 45.0);
        meter.ingest(flash(500));
        assert_eq!(meter.state["rate"]["m"][stream][0], 200, "끊긴 버스트는 안 넣는다");

        meter.state["rate"]["h"] = json!("2020-01-01T00:00");
        meter.ingest(flash(1));
        let rates = data_dir().join("rates.jsonl");
        let done: Value = serde_json::from_str(fs::read_to_string(&rates).unwrap().lines().next().unwrap()).unwrap();
        assert_eq!(done["h"], "2020-01-01T00:00");
        assert_eq!(done["m"][stream][0], 200);

        meter.reset_stats();
        assert_eq!(meter.state["rate"]["m"], json!({}));
        assert!(!rates.exists());
    }

    #[test]
    fn live_file_supplies_project_and_token_flow_keeps_it_alive() {
        let (_g, _tmp) = crate::test_home("live");
        let mut meter = Meter::new();
        let grok = |session: &str| TokenDelta {
            output_tokens: 10,
            model: "grok-4.6-build".into(),
            service: "grok".into(),
            session: session.into(),
            ..TokenDelta::default()
        };
        meter.add_live("grok", "sess-1", "", "/Users/dev/work/api", "", "SessionStart");
        meter.ingest(grok("sess-1"));
        assert!(meter.state["projects"].get("work/api").is_some(), "훅이 남긴 라이브 파일에서 프로젝트를 잇는다");
        assert!(meter.state["projects"].get("(unknown)").is_none());
        meter.ingest(grok("ghost"));
        assert!(meter.state["projects"].get("(unknown)").is_some(), "라이브 파일이 없으면 (unknown)");

        fs::write(
            live_path("grok", "sess-2"),
            json!({"service": "grok", "session_id": "sess-2", "project": "api", "cwd": "/Users/dev/work/acme/api"}).to_string(),
        )
        .unwrap();
        meter.ingest(grok("sess-2"));
        assert!(meter.state["projects"].get("acme/api").is_some(), "저장된 project 보다 cwd 가 진실이다");
        assert_eq!(meter.state["sessions"]["grok/sess-2"]["cwd"], "/Users/dev/work/acme/api");

        let path = meter.add_live("claude-code", "a/b c", "", "/tmp/proj", "", "manual");
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.contains('/') && !name.contains(' '), "{name}");
        let live = meter.status()["live"].clone();
        let row = live.as_array().unwrap().iter().find(|r| r["session_id"] == "a/b c").unwrap();
        assert_eq!(row["project"], "tmp/proj");
        assert!(meter.remove_live("claude-code", "a/b c"));
        assert!(!meter.remove_live("claude-code", "a/b c"));

        let path = meter.add_live("codex", "long-run", "", "", "", "SessionStart");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 3600);
        fs::File::options().write(true).open(&path).unwrap().set_modified(old).unwrap();
        meter.ingest(TokenDelta {
            output_tokens: 10,
            service: "codex".into(),
            session: "long-run".into(),
            ..TokenDelta::default()
        });
        let age = path.metadata().unwrap().modified().unwrap().elapsed().map(|d| d.as_secs()).unwrap_or(0);
        assert!(age < 60, "토큰이 들어오는 동안 라이브 파일이 prune 되면 안 된다 ({age}s)");
        meter.ingest(TokenDelta {
            output_tokens: 10,
            service: "codex".into(),
            session: "없음".into(),
            ..TokenDelta::default()
        });
    }

    #[test]
    fn pet_state_keeps_totals_and_timeline() {
        let (_g, tmp) = crate::test_home("pet");
        let path = tmp.join("state.json");
        fs::write(
            &path,
            json!({"version": 1, "pet": {
                "level": 12, "exp": 3.5, "name": "토큰햄", "born_at": 111.0, "last_fed": 222.0,
                "totals": {"input_tokens": 7, "cache_read": 1, "cache_write": 2, "output_tokens": 3, "cost_usd": 0.25}
            }})
            .to_string(),
        )
        .unwrap();
        let state = Meter::load_test(path).state;
        assert!(state.get("pet").is_none(), "펫 잔재가 남으면 안 된다");
        assert_eq!(state["version"], 2);
        assert_eq!(state["total"]["totals"]["input_tokens"], 7);
        assert_eq!(state["total"]["totals"]["cost_usd"], 0.25);
        assert_eq!(state["total"]["started_at"], 111.0, "누적 시작 시각은 born_at 을 잇는다");
        assert_eq!(state["total"]["last_seen"], 222.0);
    }

    #[test]
    fn hour_route_cells_hold_safe_labels_and_timing() {
        let (_g, _tmp) = crate::test_home("route-cells");
        let mut meter = Meter::new();
        let base = TokenDelta {
            service: "claude-code".into(),
            session: "s1".into(),
            endpoint: "https://api.anthropic.com".into(),
            plan: "subscription".into(),
            model: "claude-opus-5-5".into(),
            project: "secret-project".into(),
            cwd: "/Users/me/secret-project".into(),
            input_tokens: 10,
            output_tokens: 100,
            ..TokenDelta::default()
        };
        meter.ingest(TokenDelta { duration_ms: 2000, ..base.clone() });
        meter.ingest(TokenDelta {
            service: "my-agent".into(),
            session: "s2".into(),
            endpoint: "https://llm.corp.internal/v1".into(),
            model: "corp-gpt".into(),
            ..base.clone()
        });
        meter.ingest(TokenDelta {
            service: "my-agent".into(),
            endpoint: "https://llm.corp.internal/v1".into(),
            model: "corp-gpt".into(),
            plan: "acme-enterprise".into(),
            ..base.clone()
        });
        let r = &meter.state["hour"]["r"];
        let a = &r["claude-code\u{1f}api.anthropic.com\u{1f}subscription\u{1f}claude-opus-5"];
        assert_eq!((a[0].clone(), a[1].clone(), a[4].clone()), (json!(10), json!(100), json!(1)));
        assert_eq!((a[6].clone(), a[7].clone()), (json!(100), json!(2000)), "API 시간은 api_*로");
        assert!(r.get("other\u{1f}self-hosted\u{1f}subscription\u{1f}other").is_some(), "{r}");
        let text = r.to_string();
        assert!(!text.contains("secret-project") && !text.contains("corp"), "{text}");
        assert!(r.get("other\u{1f}self-hosted\u{1f}other\u{1f}other").is_some(), "사용자 요금제는 other: {r}");
        assert!(!text.contains("acme"), "{text}");

        meter.state["hour"]["h"] = json!("2020-01-01T00");
        meter.ingest(base.clone());
        let line = fs::read_to_string(data_dir().join("hours.jsonl")).unwrap();
        assert!(line.contains("\"r\""), "{line}");
        assert_eq!(meter.state["hour"]["r"].as_object().unwrap().len(), 1, "새 시간은 새 경로 장부");
    }

    #[test]
    fn logged_cost_wins_over_table_and_local_plan_is_free() {
        let (_g, _tmp) = crate::test_home("logged-cost");
        let table = cost_usd("claude-opus-5", 0, 1000, 0, 1000);
        assert!(table > 0.0);
        let book = |cost: Option<f64>, plan: &str| {
            let mut meter = Meter::new();
            meter.ingest(TokenDelta {
                cache_read: 1000,
                cost_usd: cost,
                plan: plan.into(),
                ..out(1000, "p")
            });
            meter.state["total"]["totals"].clone()
        };
        let cost = |cost: Option<f64>, plan: &str| number(book(cost, plan).get("cost_usd"));
        assert_eq!(cost(Some(0.5), "api"), 0.5, "로그의 비용이 가격표를 이긴다");
        assert_eq!(cost(Some(0.0), "api"), table, "0은 가격표로");
        assert_eq!(cost(None, "subscription"), table);
        assert_eq!(cost(Some(f64::NAN), "api"), table, "유한하지 않으면 가격표로");
        assert_eq!(cost(Some(0.5), "local"), 0.0, "local 요금제는 로그의 비용보다 먼저 0");
        assert_eq!(number(book(None, "local").get("cache_saved_usd")), 0.0, "공짜에서 아낀 돈은 없다");
    }

    #[test]
    fn calls_follow_the_delta() {
        let (_g, _tmp) = crate::test_home("calls-delta");
        assert_eq!(TokenDelta::default().calls, 1, "2.2 전까지 델타마다 1");
        let mut meter = Meter::new();
        let base = TokenDelta {
            service: "claude-code".into(),
            session: "s1".into(),
            ..out(10, "p")
        };
        meter.ingest(base.clone());
        meter.ingest(TokenDelta { calls: 0, ..base.clone() });
        for path in ["/total/totals", "/today/totals", "/session/totals", "/models/claude-opus-5/totals", "/sessions/claude-code~1s1/totals"] {
            let book = meter.state.pointer(path).unwrap();
            assert_eq!((book["calls"].clone(), book["output_tokens"].clone()), (json!(1), json!(20)), "{path}");
        }
        let cells = meter.state["hour"]["r"].as_object().unwrap();
        let cell = cells.values().next().unwrap();
        assert_eq!((cells.len(), cell[1].clone(), cell[4].clone()), (1, json!(20), json!(1)));
        assert_eq!(meter.state["hour"]["p"]["p"][2], 1, "시간 칸의 호출 수도");
    }

    #[test]
    fn speed_only_touches_only_timing_columns() {
        let (_g, _tmp) = crate::test_home("speed-only");
        let mut meter = Meter::new();
        let base = TokenDelta {
            service: "claude-code".into(),
            session: "s1".into(),
            endpoint: "https://api.anthropic.com".into(),
            ..out(100, "p")
        };
        meter.ingest(base.clone());
        let before = meter.state.clone();
        meter.ingest(TokenDelta {
            output_tokens: 300,
            duration_ms: 6000,
            calls: 0,
            speed_only: true,
            ..base
        });
        let mut after = meter.state.clone();
        let cells = after["hour"]["r"].as_object_mut().unwrap();
        assert_eq!(cells.len(), 1, "{cells:?}");
        let cell = cells.values_mut().next().unwrap();
        assert_eq!((cell[6].clone(), cell[7].clone()), (json!(300), json!(6000)), "api_out·api_ms에만");
        (cell[6], cell[7]) = (json!(0), json!(0));
        assert_eq!(after, before, "토큰·비용·세션·일 합계·라이브 속도는 그대로");
    }

    #[test]
    fn route_plan_keeps_local() {
        let (_g, _tmp) = crate::test_home("route-local");
        let plan = |plan: &str| {
            route_labels(&TokenDelta {
                service: "claude-code".into(),
                plan: plan.into(),
                ..TokenDelta::default()
            })[2]
                .clone()
        };
        assert_eq!(plan("local"), "local");
        assert_eq!(plan("acme-enterprise"), "other");
    }

    #[test]
    fn league_route_ignores_the_legacy_public_endpoints_list() {
        let (_g, tmp) = crate::test_home("route-public");
        let dir = tmp.join("config/tokenmeter");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("services.yaml"), "settings:\n  leaderboard:\n    public_endpoints: [\"llm.mycorp.com\"]\n").unwrap();
        let url = "https://llm.mycorp.com/v1";
        assert_eq!(crate::board::endpoint_label(url), "llm.mycorp.com", "레거시 리더보드는 그대로");
        let mut meter = Meter::new();
        meter.ingest(TokenDelta {
            service: "claude-code".into(),
            session: "s1".into(),
            endpoint: url.into(),
            plan: "api".into(),
            model: "claude-opus-5-5".into(),
            output_tokens: 10,
            ..TokenDelta::default()
        });
        let r = &meter.state["hour"]["r"];
        assert!(r.get("claude-code\u{1f}self-hosted\u{1f}api\u{1f}claude-opus-5").is_some(), "{r}");
        let upload = crate::sync::build(&json!({"hour": meter.state["hour"].clone()}), true, "", 0.0, crate::watch::now_secs() as i64).0;
        let text = serde_json::to_string(&upload).unwrap();
        assert!(!text.contains("mycorp") && text.contains("self-hosted"), "{text}");
    }
}
