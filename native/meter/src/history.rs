//! hours.jsonl / rates.jsonl → 그래프 막대. overlay 가 칠한다.

use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use tokenmeter_hook::data_dir;

pub const SPANS: [&str; 3] = ["today", "7d", "30d"];
pub const SPAN_TITLES: &[(&str, &str)] = &[("today", "오늘"), ("7d", "7일"), ("30d", "30일")];
pub const TOP_PROJECTS: usize = 6;
pub const OTHER: &str = "기타";

pub const RATE_ACTIVE_SECONDS: f64 = 30.0;
pub const RATE_SLOT_SECONDS: i64 = 15 * 60;
pub const RATE_SPANS: [&str; 4] = ["1h", "4h", "1d", "7d"];
pub const RATE_SPAN_TITLES: &[(&str, &str)] =
    &[("1h", "1시간"), ("4h", "4시간"), ("1d", "1일"), ("7d", "7일")];

#[derive(Clone, Debug, Default)]
pub struct Bar {
    pub label: String,
    pub total: f64,
    pub parts: Vec<(String, f64)>,
    pub calls: i64,
    pub tokens: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Series {
    pub bars: Vec<Bar>,
    pub peak: f64,
    pub projects: Vec<String>,
    pub total: f64,
}

#[derive(Clone, Debug, Default)]
pub struct RateRow {
    pub vendor: String,
    pub model: String,
    pub tokens: i64,
    pub active_sec: f64,
    pub rate: f64,
    pub share: f64,
}

#[derive(Clone, Debug, Default)]
pub struct RateBar {
    pub label: String,
    pub tokens: i64,
    pub active_sec: f64,
    pub rate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct RateSeries {
    pub span: String,
    pub start: f64,
    pub end: f64,
    pub shifted: bool,
    pub rows: Vec<RateRow>,
    pub bars: Vec<RateBar>,
    pub peak: f64,
    pub total_tokens: i64,
    pub total_active: f64,
}

pub type HourBucket = (String, serde_json::Map<String, Value>);
pub type RateBucket = (String, serde_json::Map<String, Value>);

fn hours_path() -> PathBuf {
    data_dir().join("hours.jsonl")
}
fn rates_path() -> PathBuf {
    data_dir().join("rates.jsonl")
}

fn as_f64(v: &Value) -> f64 {
    match v {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn as_i64(v: &Value) -> i64 {
    as_f64(v) as i64
}

fn finite(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

// ── 로컬 시각. hours 키는 벽시계 날짜라 UTC 나눗셈이면 칸이 하루 밀린다.
#[repr(C)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const i8,
}

extern "C" {
    fn localtime_r(clock: *const i64, result: *mut Tm) -> *mut Tm;
    fn mktime(timeptr: *mut Tm) -> i64;
}

#[derive(Clone, Copy)]
pub struct Civil {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub isdst: i32,
}

pub fn civil_of(ts: f64) -> Civil {
    let t = ts as i64;
    let mut tm = unsafe { std::mem::zeroed() };
    let p = unsafe { localtime_r(&t, &mut tm) };
    if p.is_null() {
        return Civil {
            year: 1970,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            isdst: 0,
        };
    }
    Civil {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        isdst: tm.tm_isdst,
    }
}

pub fn mktime_local(year: i32, month: u32, day: u32, hour: u32, minute: u32, isdst: i32) -> f64 {
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day as i32;
    tm.tm_hour = hour as i32;
    tm.tm_min = minute as i32;
    tm.tm_isdst = isdst;
    unsafe { mktime(&mut tm) as f64 }
}

fn add_days(c: Civil, delta: i32) -> Civil {
    let ts = mktime_local(c.year, c.month, c.day, 12, 0, -1) + f64::from(delta) * 86400.0;
    civil_of(ts)
}

fn ymd(c: Civil) -> String {
    format!("{:04}-{:02}-{:02}", c.year, c.month, c.day)
}

fn md(c: Civil) -> String {
    format!("{:02}-{:02}", c.month, c.day)
}

struct FileCache<T> {
    key: Option<(String, u64, u64)>,
    rows: T,
}

fn load_jsonl<T, F>(path: PathBuf, cache: &Mutex<FileCache<Vec<T>>>, parse: F) -> Vec<T>
where
    T: Clone,
    F: Fn(&Value) -> Option<T>,
{
    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(_) => {
            if let Ok(mut g) = cache.lock() {
                g.key = None;
                g.rows.clear();
            }
            return Vec::new();
        }
    };
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let key = (path.to_string_lossy().into_owned(), mtime, meta.len());
    if let Ok(g) = cache.lock() {
        if g.key.as_ref() == Some(&key) {
            return g.rows.clone();
        }
    }
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(row) = parse(&rec) {
            rows.push(row);
        }
    }
    if let Ok(mut g) = cache.lock() {
        g.key = Some(key);
        g.rows = rows.clone();
    }
    rows
}

fn hours_cache() -> &'static Mutex<FileCache<Vec<HourBucket>>> {
    static CACHE: Mutex<FileCache<Vec<HourBucket>>> = Mutex::new(FileCache {
        key: None,
        rows: Vec::new(),
    });
    &CACHE
}

fn rates_cache() -> &'static Mutex<FileCache<Vec<RateBucket>>> {
    static CACHE: Mutex<FileCache<Vec<RateBucket>>> = Mutex::new(FileCache {
        key: None,
        rows: Vec::new(),
    });
    &CACHE
}

fn parse_hour(rec: &Value) -> Option<HourBucket> {
    let h = rec.get("h")?.as_str()?.to_string();
    if h.is_empty() {
        return None;
    }
    let p = rec.get("p")?.as_object()?.clone();
    Some((h, p))
}

fn parse_rate(rec: &Value) -> Option<RateBucket> {
    let h = rec.get("h")?.as_str()?.to_string();
    if h.is_empty() {
        return None;
    }
    let m = rec.get("m")?.as_object()?.clone();
    Some((h, m))
}

pub fn load_hours(state: Option<&Value>) -> Vec<HourBucket> {
    let mut rows = load_jsonl(hours_path(), hours_cache(), parse_hour);
    if let Some(node) = state.and_then(|s| s.get("hour")) {
        if let (Some(h), Some(p)) = (
            node.get("h").and_then(Value::as_str),
            node.get("p").and_then(Value::as_object),
        ) {
            if rows.last().map(|(k, _)| k.as_str()) != Some(h) {
                rows.push((h.to_string(), p.clone()));
            }
        }
    }
    rows
}

pub fn load_rates(state: Option<&Value>) -> Vec<RateBucket> {
    let mut rows = load_jsonl(rates_path(), rates_cache(), parse_rate);
    if let Some(node) = state.and_then(|s| s.get("rate")) {
        if let (Some(h), Some(m)) = (
            node.get("h").and_then(Value::as_str),
            node.get("m").and_then(Value::as_object),
        ) {
            if rows.last().map(|(k, _)| k.as_str()) != Some(h) {
                rows.push((h.to_string(), m.clone()));
            }
        }
    }
    rows
}

fn slots(span: &str, now: f64) -> Vec<(String, String)> {
    let today = civil_of(now);
    if span == "today" {
        let stamp = ymd(today);
        return (0..24)
            .map(|h| (format!("{stamp}T{h:02}"), format!("{h:02}")))
            .collect();
    }
    let days = if span == "7d" { 7 } else { 30 };
    (0..days)
        .rev()
        .map(|i| {
            let d = add_days(today, -(i as i32));
            (ymd(d), md(d))
        })
        .collect()
}

fn key_of(hour: &str, span: &str) -> String {
    if span == "today" {
        hour.to_string()
    } else {
        hour.chars().take(10).collect()
    }
}

fn fold(parts: Vec<(String, f64)>, top: &[String]) -> Vec<(String, f64)> {
    let mut merged: HashMap<String, f64> = HashMap::new();
    for (name, value) in parts {
        let key = if top.iter().any(|t| t == &name) {
            name
        } else {
            OTHER.into()
        };
        *merged.entry(key).or_insert(0.0) += value;
    }
    let order: HashMap<&str, usize> = top
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();
    let mut out: Vec<(String, f64)> = merged.into_iter().collect();
    out.sort_by(|a, b| {
        order
            .get(a.0.as_str())
            .copied()
            .unwrap_or(top.len())
            .cmp(&order.get(b.0.as_str()).copied().unwrap_or(top.len()))
    });
    out
}

pub fn series(hours: &[HourBucket], span: &str, project: Option<&str>, now: f64) -> Series {
    let span = if SPANS.contains(&span) { span } else { "today" };
    let slot_list = slots(span, now);
    let mut index: HashMap<String, Bar> = slot_list
        .iter()
        .map(|(k, label)| {
            (
                k.clone(),
                Bar {
                    label: label.clone(),
                    ..Bar::default()
                },
            )
        })
        .collect();
    let mut weight: HashMap<String, f64> = HashMap::new();
    for (hour, book) in hours {
        let Some(bar) = index.get_mut(&key_of(hour, span)) else {
            continue;
        };
        for (name, cell) in book {
            if let Some(want) = project {
                if name != want {
                    continue;
                }
            }
            let parts = match cell {
                Value::Array(a) if a.len() >= 3 => a,
                _ => continue,
            };
            let cost = as_f64(&parts[1]);
            bar.total += cost;
            bar.tokens += as_i64(&parts[0]);
            bar.calls += as_i64(&parts[2]);
            bar.parts.push((name.clone(), cost));
            *weight.entry(name.clone()).or_insert(0.0) += cost;
        }
    }
    let mut ranked: Vec<String> = weight.keys().cloned().collect();
    ranked.sort_by(|a, b| {
        weight[b]
            .partial_cmp(&weight[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let top: Vec<String> = ranked.iter().take(TOP_PROJECTS).cloned().collect();
    for bar in index.values_mut() {
        bar.parts = fold(std::mem::take(&mut bar.parts), &top);
    }
    let bars: Vec<Bar> = slot_list
        .iter()
        .filter_map(|(k, _)| index.remove(k))
        .collect();
    let peak = bars.iter().map(|b| b.total).fold(0.0, f64::max);
    let total = (bars.iter().map(|b| b.total).sum::<f64>() * 1_000_000.0).round() / 1_000_000.0;
    let mut projects = top;
    if ranked.len() > TOP_PROJECTS {
        projects.push(OTHER.into());
    }
    Series {
        bars,
        peak,
        projects,
        total,
    }
}

fn comma_int(n: i64) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    let digits: String = out.chars().rev().collect();
    if n < 0 {
        format!("-{digits}")
    } else {
        digits
    }
}

fn comma_money(v: f64) -> String {
    let rounded = (v * 100.0).round() / 100.0;
    let int = rounded.trunc() as i64;
    let frac = ((rounded - int as f64).abs() * 100.0).round() as i64;
    format!("{}.{:02}", comma_int(int), frac)
}

pub fn summary(s: &Series) -> String {
    if s.peak <= 0.0 {
        return String::new();
    }
    let calls: i64 = s.bars.iter().map(|b| b.calls).sum();
    format!("${} · {}호출", comma_money(s.total), comma_int(calls))
}

pub fn rate_slot(ts: f64) -> String {
    let c = civil_of(ts);
    let minute = c.minute - (c.minute % 15);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}",
        c.year, c.month, c.day, c.hour, minute
    )
}

pub fn slot_start(key: &str) -> f64 {
    let bytes = key.as_bytes();
    if bytes.len() < 16 {
        return 0.0;
    }
    let Ok(year) = key[0..4].parse::<i32>() else {
        return 0.0;
    };
    let Ok(month) = key[5..7].parse::<u32>() else {
        return 0.0;
    };
    let Ok(day) = key[8..10].parse::<u32>() else {
        return 0.0;
    };
    let Ok(hour) = key[11..13].parse::<u32>() else {
        return 0.0;
    };
    let Ok(minute) = key[14..16].parse::<u32>() else {
        return 0.0;
    };
    mktime_local(year, month, day, hour, minute, -1)
}

pub fn slot_end(key: &str) -> f64 {
    let start = slot_start(key);
    if start == 0.0 {
        0.0
    } else {
        start + RATE_SLOT_SECONDS as f64
    }
}

pub fn rate_key(vendor: &str, model: &str) -> String {
    let v = if vendor.trim().is_empty() {
        "unknown"
    } else {
        vendor.trim()
    };
    let m = if model.trim().is_empty() {
        "default"
    } else {
        model.trim()
    };
    format!("{v}/{m}")
}

pub fn split_rate_key(name: &str) -> (String, String) {
    match name.split_once('/') {
        Some((v, m)) => (
            if v.is_empty() { "unknown".into() } else { v.into() },
            if m.is_empty() { "default".into() } else { m.into() },
        ),
        None => ("unknown".into(), if name.is_empty() { "default".into() } else { name.into() }),
    }
}

fn cell_parts(cell: &Value) -> (i64, f64) {
    let Value::Array(a) = cell else {
        return (0, 0.0);
    };
    if a.len() < 2 {
        return (0, 0.0);
    }
    (as_i64(&a[0]).max(0), finite(as_f64(&a[1])).max(0.0))
}

fn tokens_in(buckets: &[RateBucket], start: f64, end: f64) -> i64 {
    let mut total = 0;
    for (key, book) in buckets {
        let t = slot_start(key);
        if t < start || t >= end {
            continue;
        }
        total += book.values().map(cell_parts).map(|(n, _)| n).sum::<i64>();
    }
    total
}

fn latest_end(buckets: &[RateBucket]) -> f64 {
    let mut latest: f64 = 0.0;
    for (key, book) in buckets {
        if book.values().any(|c| cell_parts(c).0 > 0) {
            latest = latest.max(slot_end(key));
        }
    }
    latest
}

fn rate_window(span: &str, now: f64, buckets: &[RateBucket]) -> (f64, f64, bool) {
    let width = match span {
        "4h" => 4.0 * 3600.0,
        "1d" => 24.0 * 3600.0,
        "7d" => 7.0 * 24.0 * 3600.0,
        _ => 3600.0,
    };
    let end = now;
    let start = end - width;
    if tokens_in(buckets, start, end) > 0 {
        return (start, end, false);
    }
    let last = latest_end(buckets);
    if last <= 0.0 {
        return (start, end, false);
    }
    (last - width, last, true)
}

fn bar_label(span: &str, t: f64) -> String {
    let c = civil_of(t);
    match span {
        "7d" => md(c),
        "1d" => format!("{:02}", c.hour),
        _ => format!("{:02}:{:02}", c.hour, c.minute),
    }
}

fn align(ts: f64, step: i64) -> f64 {
    let c = civil_of(ts);
    if step >= 24 * 3600 {
        mktime_local(c.year, c.month, c.day, 0, 0, c.isdst)
    } else if step >= 3600 {
        mktime_local(c.year, c.month, c.day, c.hour, 0, c.isdst)
    } else {
        let minute = c.minute - (c.minute % (step as u32 / 60).max(1));
        mktime_local(c.year, c.month, c.day, c.hour, minute, c.isdst)
    }
}

fn rate_bar_step(span: &str) -> i64 {
    match span {
        "1d" => 3600,
        "7d" => 24 * 3600,
        _ => RATE_SLOT_SECONDS,
    }
}

pub fn rate_series(buckets: &[RateBucket], span: &str, now: f64) -> RateSeries {
    let span = if RATE_SPANS.contains(&span) { span } else { "1h" };
    let (start, end, shifted) = rate_window(span, now, buckets);
    let step = rate_bar_step(span);
    let origin = align(start, step);
    let mut bars = Vec::new();
    let mut index: HashMap<i64, usize> = HashMap::new();
    let mut t = origin;
    while t < end {
        index.insert(t as i64, bars.len());
        bars.push(RateBar {
            label: bar_label(span, t),
            ..RateBar::default()
        });
        t += step as f64;
    }
    let mut weight: HashMap<String, (f64, f64)> = HashMap::new();
    for (key, book) in buckets {
        let slot = slot_start(key);
        if slot < start || slot >= end {
            continue;
        }
        let bar_at = align(slot, step) as i64;
        let bar_i = index.get(&bar_at).copied();
        for (name, cell) in book {
            let (tokens, active) = cell_parts(cell);
            if tokens <= 0 && active <= 0.0 {
                continue;
            }
            let node = weight.entry(name.clone()).or_insert((0.0, 0.0));
            node.0 += tokens as f64;
            node.1 += active;
            if let Some(i) = bar_i {
                bars[i].tokens += tokens;
                bars[i].active_sec += active;
            }
        }
    }
    for bar in &mut bars {
        bar.rate = if bar.active_sec > 0.0 {
            bar.tokens as f64 / bar.active_sec
        } else {
            0.0
        };
    }
    let total_tokens = weight.values().map(|v| v.0).sum::<f64>() as i64;
    let total_active = weight.values().map(|v| v.1).sum::<f64>();
    let mut rows: Vec<RateRow> = weight
        .into_iter()
        .map(|(name, (tokens, active))| {
            let (vendor, model) = split_rate_key(&name);
            RateRow {
                vendor,
                model,
                tokens: tokens as i64,
                active_sec: active,
                rate: if active > 0.0 { tokens / active } else { 0.0 },
                share: if total_tokens > 0 {
                    tokens / total_tokens as f64
                } else {
                    0.0
                },
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.tokens
            .cmp(&a.tokens)
            .then(a.vendor.cmp(&b.vendor))
            .then(a.model.cmp(&b.model))
    });
    let peak = bars.iter().map(|b| b.rate).fold(0.0, f64::max);
    RateSeries {
        span: span.into(),
        start,
        end,
        shifted,
        rows,
        bars,
        peak,
        total_tokens,
        total_active,
    }
}

pub fn rate_summary(s: &RateSeries) -> String {
    if s.total_active <= 0.0 {
        return "아직 작업 속도 기록이 없습니다".into();
    }
    let avg = s.total_tokens as f64 / s.total_active;
    let c = civil_of(s.start);
    let when = format!("{:02}-{:02} {:02}:{:02}", c.month, c.day, c.hour, c.minute);
    let prefix = if s.shifted { "마지막 기록 · " } else { "" };
    format!(
        "{prefix}{avg:.1} tok/s · {}토큰 · {when}",
        comma_int(s.total_tokens as i64)
    )
}

pub fn day_noon(day: &str) -> f64 {
    if day.len() < 10 {
        return crate::watch::now_secs();
    }
    let Ok(y) = day[0..4].parse::<i32>() else {
        return crate::watch::now_secs();
    };
    let Ok(m) = day[5..7].parse::<u32>() else {
        return crate::watch::now_secs();
    };
    let Ok(d) = day[8..10].parse::<u32>() else {
        return crate::watch::now_secs();
    };
    mktime_local(y, m, d, 12, 0, -1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn series_matches_python_demo() {
        let stamp = mktime_local(2026, 8, 11, 15, 0, -1);
        let rows = vec![
            (
                "2026-08-11T09".into(),
                json!({"a": [100, 1.0, 2]}).as_object().unwrap().clone(),
            ),
            (
                "2026-08-11T15".into(),
                json!({"a": [50, 2.0, 1], "b": [10, 0.5, 1]})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            (
                "2026-08-10T15".into(),
                json!({"a": [70, 4.0, 3]}).as_object().unwrap().clone(),
            ),
        ];
        let s = series(&rows, "today", None, stamp);
        assert_eq!(s.bars.len(), 24);
        assert_eq!(s.bars[0].label, "00");
        assert!((s.bars[9].total - 1.0).abs() < 1e-9);
        assert!((s.bars[15].total - 2.5).abs() < 1e-9);
        assert_eq!(s.bars[15].calls, 2);
        assert_eq!(s.bars[15].tokens, 60);
        assert!((s.peak - 2.5).abs() < 1e-9);
        assert!((s.total - 3.5).abs() < 1e-9);
        assert_eq!(s.bars[0].total, 0.0);

        let d = series(&rows, "7d", None, stamp);
        assert_eq!(d.bars.len(), 7);
        assert_eq!(d.bars.last().unwrap().label, "08-11");
        assert!((d.bars.last().unwrap().total - 3.5).abs() < 1e-9);
        assert!((d.bars[d.bars.len() - 2].total - 4.0).abs() < 1e-9);
        assert_eq!(d.projects, vec!["a", "b"]);

        let only = series(&rows, "today", Some("b"), stamp);
        assert!((only.total - 0.5).abs() < 1e-9);
        assert_eq!(only.bars[15].parts, vec![("b".into(), 0.5)]);

        let mut many_map = serde_json::Map::new();
        for i in 0..9 {
            many_map.insert(format!("p{i}"), json!([1, 10.0 - i as f64, 1]));
        }
        let many = vec![("2026-08-11T15".into(), many_map)];
        let m = series(&many, "today", None, stamp);
        assert_eq!(m.projects.last().unwrap(), OTHER);
        assert_eq!(m.projects.len(), TOP_PROJECTS + 1);
        assert_eq!(m.bars[15].parts.last().unwrap().0, OTHER);
        assert!((m.bars[15].parts.last().unwrap().1 - 9.0).abs() < 1e-9);

        let empty = series(&[], "30d", None, stamp);
        assert_eq!(empty.bars.len(), 30);
        assert_eq!(empty.peak, 0.0);
        assert_eq!(summary(&empty), "");
    }

    #[test]
    fn load_hours_skips_broken_and_dedups_live() {
        let _g = crate::TEST_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("tm-hist-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        std::env::set_var("TOKENMETER_HOME", &dir);
        if let Ok(mut c) = hours_cache().lock() {
            c.key = None;
            c.rows.clear();
        }
        fs::write(
            dir.join("hours.jsonl"),
            "{\"h\":\"2026-08-11T16\",\"p\":{\"a\":[1,1.0,1]}}\n깨진 줄\n{\"h\":\"nope\"}\n",
        )
        .unwrap();
        assert_eq!(load_hours(None).len(), 1);
        let live = json!({"a": [1, 1.0, 1]});
        assert_eq!(
            load_hours(Some(&json!({"hour": {"h": "2026-08-11T16", "p": live}}))).len(),
            1
        );
        assert_eq!(
            load_hours(Some(&json!({"hour": {"h": "2026-08-11T17", "p": live}}))).len(),
            2
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rate_slot_is_15_minute_local() {
        let ts = mktime_local(2026, 8, 14, 15, 7, -1);
        assert_eq!(rate_slot(ts), "2026-08-14T15:00");
        let ts = mktime_local(2026, 8, 14, 15, 29, -1);
        assert_eq!(rate_slot(ts), "2026-08-14T15:15");
    }
}
