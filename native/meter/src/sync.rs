//! 리그 동기화. 엔진은 토큰이 들어온 시각의 칸에 쌓으므로 지난 칸은 다시 바뀌지 않는다.
//! 그래서 업로드마다 `synced_through` 뒤의 지난 칸과 지금 칸만 보내고, 같은 본문을
//! 두 번 보내도 서버 결과는 같다.

use crate::history::{civil_of, mktime_local};
use crate::server::{self, ApiError};
use crate::VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokenmeter_hook::data_dir;
use tokenmeter_protocol::{Cell, HourCells, UsageUpload, ALL, MAX_HOURS, PAST_SECS, VERSION as WIRE};

const ACTIVE_S: f64 = 60.0;
const IDLE_S: f64 = 900.0;
const CONFIG_EVERY: f64 = 6.0 * 3600.0;
const LOOK_EVERY: f64 = 5.0;

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SyncState {
    /// 서버가 가진 마지막 지난 칸 키("YYYY-MM-DDTHH").
    pub synced_through: String,
    pub last_ok: f64,
    pub fails: u32,
    pub retry_at: f64,
    /// 한 번에 다 못 보낸 지난 칸이 남아 있다.
    pub more: bool,
    pub active_s: f64,
    pub idle_s: f64,
    #[serde(skip)]
    pub config_at: f64,
    /// 426을 받은 클라이언트 버전. 새 바이너리가 뜰 때까지 멈춘다.
    pub upgrade_for: String,
}

static RUNNING: AtomicBool = AtomicBool::new(false);
static STATE: OnceLock<Mutex<SyncState>> = OnceLock::new();
static NEXT_LOOK: Mutex<f64> = Mutex::new(0.0);

fn path() -> std::path::PathBuf {
    data_dir().join("league-sync.json")
}

pub fn load() -> SyncState {
    fs::read_to_string(path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save(state: &SyncState) {
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(path(), serde_json::to_string(state).unwrap_or_default());
}

fn state() -> &'static Mutex<SyncState> {
    STATE.get_or_init(|| Mutex::new(load()))
}

pub fn forget() {
    let _ = fs::remove_file(path());
    *state().lock().unwrap() = SyncState::default();
}

/// 로컬 시각 키 "YYYY-MM-DDTHH"가 시작하는 유닉스 초.
pub fn hour_start(key: &str) -> Option<i64> {
    let part = |a: usize, b: usize| key.get(a..b)?.parse::<u32>().ok();
    let (y, m, d, h) = (part(0, 4)?, part(5, 7)?, part(8, 10)?, part(11, 13)?);
    Some(mktime_local(y as i32, m, d, h, 0, -1) as i64)
}

fn cells(book: &Value, share: bool) -> Vec<Cell> {
    let mut out = Vec::new();
    for (key, v) in book.as_object().into_iter().flatten() {
        let n = |i: usize| v.get(i).and_then(Value::as_u64).unwrap_or(0);
        let parts: Vec<&str> = key.split('\u{1f}').collect();
        let [client, route, plan, model] = parts.as_slice() else { continue };
        out.push(Cell {
            client: client.to_string(),
            route: route.to_string(),
            plan: plan.to_string(),
            model: model.to_string(),
            input_tokens: n(0),
            output_tokens: n(1),
            cache_read: n(2),
            cache_write: n(3),
            calls: n(4),
            cost_usd: v.get(5).and_then(Value::as_f64).unwrap_or(0.0),
            api_out: n(6),
            api_ms: n(7),
            gap_out: n(8),
            gap_ms: n(9),
        });
    }
    if share || out.is_empty() {
        return out;
    }
    let mut total = Cell { client: ALL.into(), route: ALL.into(), plan: ALL.into(), model: ALL.into(), ..Cell::default() };
    for c in &out {
        total.input_tokens += c.input_tokens;
        total.output_tokens += c.output_tokens;
        total.cache_read += c.cache_read;
        total.cache_write += c.cache_write;
        total.calls += c.calls;
        total.cost_usd += c.cost_usd;
        total.api_out += c.api_out;
        total.api_ms += c.api_ms;
        total.gap_out += c.gap_out;
        total.gap_ms += c.gap_ms;
    }
    vec![total]
}

/// 다음 업로드: `synced_through` 뒤의 지난 칸(오래된 것부터, 최대 MAX_HOURS)과,
/// 자리가 남으면 `state.hour`의 지금 칸. 새 커서와 아직 남은 칸이 있는지도 돌려준다.
pub fn build(state_now: &Value, share: bool, synced_through: &str, now: i64) -> (UsageUpload, String, bool) {
    let mut hours = Vec::new();
    let mut cursor = synced_through.to_string();
    let mut more = false;
    let text = fs::read_to_string(data_dir().join("hours.jsonl")).unwrap_or_default();
    for line in text.lines() {
        let Ok(rec) = serde_json::from_str::<Value>(line) else { continue };
        let Some(h) = rec.get("h").and_then(Value::as_str) else { continue };
        if h <= synced_through {
            continue;
        }
        if hours.len() == MAX_HOURS {
            more = true;
            break;
        }
        cursor = h.to_string();
        let Some(t) = hour_start(h) else { continue };
        if t < now - PAST_SECS + 3600 {
            continue;
        }
        let cells = cells(rec.get("r").unwrap_or(&Value::Null), share);
        if !cells.is_empty() {
            hours.push(HourCells { t, cells });
        }
    }
    if !more && hours.len() < MAX_HOURS {
        // 데몬이 오래 멈췄다 뜨면 지금 칸이 30일 밖일 수 있다. 서버가 400으로 막으므로 빼고 보낸다.
        if let Some(t) = state_now
            .pointer("/hour/h")
            .and_then(Value::as_str)
            .and_then(hour_start)
            .filter(|&t| t >= now - PAST_SECS + 3600)
        {
            let cells = cells(state_now.pointer("/hour/r").unwrap_or(&Value::Null), share);
            if !cells.is_empty() {
                hours.push(HourCells { t, cells });
            }
        }
    }
    (UsageUpload { v: WIRE, share, endpoint_id: None, hours }, cursor, more)
}

fn every(v: f64, default: f64) -> f64 {
    if v > 0.0 {
        v.clamp(30.0, 3600.0)
    } else {
        default
    }
}

fn backoff(fails: u32) -> f64 {
    (60.0 * 2f64.powi(fails.clamp(1, 5) as i32 - 1)).min(900.0)
}

pub fn due(s: &SyncState, now: f64, last_seen: f64) -> bool {
    if s.upgrade_for == VERSION || now < s.retry_at {
        return false;
    }
    if s.last_ok == 0.0 || s.more {
        return true;
    }
    let wait = if last_seen > s.last_ok { every(s.active_s, ACTIVE_S) } else { every(s.idle_s, IDLE_S) };
    now - s.last_ok >= wait
}

/// M1: 공유가 켜져 있을 때만 보낸다. M2에서 "리그에 로그인했을 때"가 더해진다.
pub fn wanted() -> bool {
    crate::share::on() && !server::base().is_empty()
}

/// 데몬 루프에서 부른다. 5초에 한 번만 보고, 전송은 따로 스레드에서 한다.
pub fn tick(status: &Value) {
    let now = crate::watch::now_secs();
    {
        let mut next = NEXT_LOOK.lock().unwrap();
        if now < *next {
            return;
        }
        *next = now + LOOK_EVERY;
    }
    let last_seen = status.pointer("/total/last_seen").and_then(Value::as_f64).unwrap_or(0.0);
    let is_due = due(&state().lock().unwrap(), now, last_seen);
    if !is_due || !wanted() || RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let hour = status.get("hour").cloned().unwrap_or(Value::Null);
    std::thread::spawn(move || {
        let _ = run_once(&serde_json::json!({ "hour": hour }), server::NORMAL);
        RUNNING.store(false, Ordering::SeqCst);
    });
}

/// 업로드 한 번. 상태를 고치고 저장한다. 보낸 칸 수를 돌려준다.
pub fn run_once(state_now: &Value, timeout: Duration) -> Result<usize, ApiError> {
    let now = crate::watch::now_secs();
    let mut s = state().lock().unwrap().clone();
    // 종료 때의 5초 전송(flush)에서는 config를 받지 않는다. 그 시간은 사용량에 쓴다.
    if timeout >= server::NORMAL && now - s.config_at >= CONFIG_EVERY {
        if let Ok(cfg) = server::config() {
            s.active_s = cfg.active_s as f64;
            s.idle_s = cfg.idle_s as f64;
            s.config_at = now;
        }
    }
    let result = server::ensure_device().and_then(|token| -> Result<(usize, String, bool), ApiError> {
        let (upload, cursor, more) = build(state_now, crate::share::on(), &s.synced_through, now as i64);
        if !upload.hours.is_empty() {
            server::put_usage(&token, &upload, timeout)?;
        }
        Ok((upload.hours.len(), cursor, more))
    });
    let out = match result {
        Ok((sent, cursor, more)) => {
            s.synced_through = cursor;
            s.last_ok = now;
            s.more = more;
            s.fails = 0;
            s.retry_at = 0.0;
            Ok(sent)
        }
        Err(e) => {
            match &e {
                ApiError::Upgrade => s.upgrade_for = VERSION.into(),
                ApiError::Status(401, _) => {
                    server::forget_device();
                    s.retry_at = now + 60.0;
                }
                _ => {
                    s.fails += 1;
                    s.retry_at = now + backoff(s.fails);
                }
            }
            Err(e)
        }
    };
    save(&s);
    *state().lock().unwrap() = s;
    out
}

/// 데몬이 멈출 때 마지막 전송. 지난 전송 뒤 바뀐 게 있을 때만, 5초 안에.
pub fn flush(state_now: &Value) {
    let last_seen = state_now.pointer("/total/last_seen").and_then(Value::as_f64).unwrap_or(0.0);
    let s = state().lock().unwrap().clone();
    // 기기 토큰이 없으면 발급(15초)부터 해야 하니 종료 때는 하지 않는다.
    if !wanted()
        || s.upgrade_for == VERSION
        || last_seen <= s.last_ok
        || server::device_token().is_none()
        || RUNNING.swap(true, Ordering::SeqCst)
    {
        return;
    }
    let _ = run_once(state_now, Duration::from_secs(5));
    RUNNING.store(false, Ordering::SeqCst);
}

/// 공유를 켰을 때 다음 업로드에 들어갈 JSON.
pub fn preview(state_now: &Value) -> String {
    let (upload, _, _) = build(state_now, true, &load().synced_through, crate::watch::now_secs() as i64);
    serde_json::to_string_pretty(&upload).unwrap_or_default()
}

pub fn hour_key(ts: f64) -> String {
    let c = civil_of(ts);
    format!("{:04}-{:02}-{:02}T{:02}", c.year, c.month, c.day, c.hour)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokenmeter_protocol::validate;

    fn book(model: &str, out: i64) -> Value {
        json!({ format!("claude-code\u{1f}api.anthropic.com\u{1f}subscription\u{1f}{model}"): [1, out, 0, 0, 1, 0.5, 0, 0, out, 900] })
    }

    #[test]
    fn upload_carries_new_finished_hours_and_the_current_hour() {
        let (_g, _tmp) = crate::test_home("sync-build");
        let now = crate::watch::now_secs();
        let (h1, h2, h3, h4) = (hour_key(now - 4.0 * 3600.0), hour_key(now - 3.0 * 3600.0), hour_key(now - 2.0 * 3600.0), hour_key(now));
        let lines = [
            json!({"h": h1, "p": {"x": [1, 0.1, 1]}, "r": book("claude-opus-5", 10)}),
            json!({"h": h2, "p": {"x": [1, 0.1, 1]}}),
            json!({"h": h3, "p": {"x": [1, 0.1, 1]}, "r": book("gpt-5.6-sol", 30)}),
        ];
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("hours.jsonl"), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        let state_now = json!({"hour": {"h": h4, "p": {}, "r": book("claude-opus-5", 5)}});

        let (up, cursor, more) = build(&state_now, true, &h1, now as i64);
        assert_eq!(up.hours.iter().map(|h| h.cells[0].output_tokens).collect::<Vec<_>>(), [30, 5]);
        assert_eq!((cursor.as_str(), more), (h3.as_str(), false));
        assert_eq!(validate(&up, now as i64), Ok(()));
        assert!(!serde_json::to_string(&up).unwrap().contains("\"x\""), "프로젝트 칸은 나가지 않는다");

        let (again, cursor2, _) = build(&state_now, true, &cursor, now as i64);
        assert_eq!(again.hours.len(), 1, "지금 칸만 다시 간다");
        assert_eq!(cursor2, cursor);

        let stale = json!({"hour": {"h": hour_key(now - 40.0 * 86_400.0), "r": book("claude-opus-5", 5)}});
        assert!(build(&stale, true, &cursor, now as i64).0.hours.is_empty(), "40일 전 지금 칸은 보내지 않는다");
    }

    #[test]
    fn share_off_sends_one_total_cell_per_hour() {
        let (_g, _tmp) = crate::test_home("sync-total");
        let now = crate::watch::now_secs();
        let mut both = book("claude-opus-5", 10);
        both.as_object_mut().unwrap().extend(book("gpt-5.6-sol", 20).as_object().unwrap().clone());
        let state_now = json!({"hour": {"h": hour_key(now), "r": both}});
        let (up, _, _) = build(&state_now, false, "", now as i64);
        let c = &up.hours[0].cells;
        assert_eq!((c.len(), c[0].model.as_str(), c[0].output_tokens, c[0].gap_ms), (1, ALL, 30, 1800));
        assert_eq!(validate(&up, now as i64), Ok(()));
    }

    #[test]
    fn schedule_follows_activity_backoff_and_upgrade() {
        let mut s = SyncState::default();
        assert!(due(&s, 1000.0, 0.0), "한 번도 안 보냄");
        s.last_ok = 1000.0;
        assert!(!due(&s, 1030.0, 1010.0) && due(&s, 1060.0, 1010.0), "새 토큰: 60초마다");
        assert!(!due(&s, 1800.0, 900.0) && due(&s, 1900.0, 900.0), "쉬는 중: 900초마다");
        s.retry_at = 5000.0;
        assert!(!due(&s, 4000.0, 3000.0));
        s.retry_at = 0.0;
        s.upgrade_for = VERSION.into();
        assert!(!due(&s, 9000.0, 8000.0), "426이면 새 바이너리까지 멈춤");
        assert_eq!((backoff(1), backoff(2), backoff(9)), (60.0, 120.0, 900.0));
    }

    #[test]
    fn run_once_registers_uploads_and_moves_the_cursor() {
        let (_g, _tmp) = crate::test_home("sync-run");
        forget();
        let (url, seen) = crate::server::fake::serve(vec![
            (200, r#"{"relay":"","invite":"","active_s":60,"idle_s":900,"live":false}"#),
            (201, r#"{"device_id":"d1","token":"tmd_x"}"#),
            (204, ""),
        ]);
        crate::server::fake::use_server(&url);
        crate::share::set(true);
        let now = crate::watch::now_secs();
        let state_now = json!({"hour": {"h": hour_key(now), "r": book("claude-opus-5", 7)}});
        assert_eq!(run_once(&state_now, crate::server::NORMAL), Ok(1));
        let reqs: Vec<String> = seen.try_iter().collect();
        assert!(reqs[0].starts_with("GET /v1/config") && reqs[1].starts_with("POST /v1/devices") && reqs[2].starts_with("PUT /v1/usage"), "{reqs:?}");
        assert!(reqs[2].contains("Bearer tmd_x") && reqs[2].contains("\"output_tokens\":7"), "{}", reqs[2]);
        assert!(load().last_ok > 0.0);
    }
}
