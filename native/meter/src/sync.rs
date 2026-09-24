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
use tokenmeter_protocol::{validate, Cell, HourCells, UsageUpload, ALL, MAX_HOURS, PAST_SECS, VERSION as WIRE};

const ACTIVE_S: f64 = 60.0;
const IDLE_S: f64 = 900.0;
const CONFIG_EVERY: f64 = 6.0 * 3600.0;
const LOOK_EVERY: f64 = 5.0;
/// 밀린 칸을 나눠 보낼 때의 간격. 서버는 기기당 분당 2회만 받는다.
const MORE_S: f64 = 30.0;
/// 본문이 이만큼을 넘기 전에 멈춘다. 서버는 256 KB가 넘으면 400으로 막는다.
const MAX_BODY: usize = 200_000;

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

/// 쓰다 끊겨도 빈 파일이 남지 않게 임시 파일에 쓰고 바꾼다. 디렉터리는 만들지 않는다:
/// `uninstall --purge`가 지운 뒤 종료 중인 데몬이 되살리지 않게(데몬은 시작할 때 만든다).
fn save(state: &SyncState) {
    let tmp = path().with_extension("tmp");
    if fs::write(&tmp, serde_json::to_string(state).unwrap_or_default()).is_ok() {
        let _ = fs::rename(tmp, path());
    }
}

fn state() -> &'static Mutex<SyncState> {
    STATE.get_or_init(|| Mutex::new(load()))
}

// ponytail: 이 프로세스의 상태만 비운다. CLI에서 부르면 실행 중인 데몬은 옛 커서를 들고 있다가
// 다시 저장한다(공유가 꺼져 있으니 보내지는 않음). 데몬에 알리려면 파일 감시나 신호가 필요하다.
pub fn forget() {
    let _ = fs::remove_file(path());
    *state().lock().unwrap() = SyncState::default();
}

/// 로컬 시각 키 "YYYY-MM-DDTHH"가 시작하는 유닉스 초.
// ponytail: DST가 되돌아가는 시각은 같은 키가 두 번 있어 t가 모호하다(glibc는 앞선 mktime에 따라
// 고른다). 드물어서 그대로 둔다. 엔진이 칸을 열 때 t를 적으면 풀린다.
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

/// 칸 하나를 더한다. 서버가 400으로 막을 칸(시간대 이동으로 생긴 미래 t, 셀 200개 초과 등)은
/// 버리고, 같은 t는 뒤의 것만 남긴다. 한 칸 때문에 묶음 전체가 거부되지 않게 한다.
/// 본문이 MAX_BODY를 넘게 되면 더하지 않고 false를 돌려준다.
fn add(hours: &mut Vec<HourCells>, size: &mut usize, hour: HourCells, share: bool, now: i64) -> bool {
    let mut one = UsageUpload { v: WIRE, share, endpoint_id: None, hours: vec![hour] };
    if one.hours[0].cells.is_empty() || validate(&one, now).is_err() {
        return true;
    }
    let n = serde_json::to_string(&one.hours[0]).map_or(0, |s| s.len() + 1);
    if *size + n > MAX_BODY {
        return false;
    }
    *size += n;
    let hour = one.hours.remove(0);
    hours.retain(|h| h.t != hour.t);
    hours.push(hour);
    true
}

/// 다음 업로드: `synced_through` 뒤의 지난 칸(오래된 것부터, 최대 MAX_HOURS)과,
/// 자리가 남으면 `state.hour`의 지금 칸. 새 커서와 아직 남은 칸이 있는지도 돌려준다.
/// 공유를 켜기 전에 끝난 칸은 보내지 않고 커서만 지나간다. 지금 칸은 켠 시간의 칸이라 보낸다.
pub fn build(state_now: &Value, share: bool, synced_through: &str, now: i64) -> (UsageUpload, String, bool) {
    let since = crate::share::since();
    let mut hours = Vec::new();
    let mut size = 0;
    let mut cursor = synced_through.to_string();
    let mut more = false;
    let text = fs::read_to_string(data_dir().join("hours.jsonl")).unwrap_or_default();
    for line in text.lines() {
        let Ok(rec) = serde_json::from_str::<Value>(line) else { continue };
        let Some(h) = rec.get("h").and_then(Value::as_str) else { continue };
        // ponytail: 커서는 키 문자열 비교다. 서쪽으로 시간대를 옮기면 이미 지난 키보다 작은 새 칸은
        // 건너뛰고, 옮기기 전 칸은 미래 t가 되어 add가 버린다(400은 막지만 그 칸은 사라짐).
        // 엔진이 칸마다 t를 적으면 t로 비교한다.
        if h <= synced_through {
            continue;
        }
        if hours.len() == MAX_HOURS {
            more = true;
            break;
        }
        let t = hour_start(h).filter(|&t| t >= now - PAST_SECS + 3600 && (t + 3600) as f64 > since);
        if let Some(t) = t {
            let hour = HourCells { t, cells: cells(rec.get("r").unwrap_or(&Value::Null), share) };
            if !add(&mut hours, &mut size, hour, share, now) {
                more = true;
                break;
            }
        }
        cursor = h.to_string();
    }
    if !more && hours.len() < MAX_HOURS {
        // 데몬이 오래 멈췄다 뜨면 지금 칸이 30일 밖일 수 있다. 서버가 400으로 막으므로 빼고 보낸다.
        // ponytail: reset 뒤의 지금 칸은 줄어든 값으로 서버의 같은 칸을 덮어쓴다(그 시간에 이미 올린
        // 토큰이 사라짐). 드물어서 그대로 둔다.
        if let Some(t) = state_now
            .pointer("/hour/h")
            .and_then(Value::as_str)
            .and_then(hour_start)
            .filter(|&t| t >= now - PAST_SECS + 3600)
        {
            let hour = HourCells { t, cells: cells(state_now.pointer("/hour/r").unwrap_or(&Value::Null), share) };
            more = !add(&mut hours, &mut size, hour, share, now);
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
    if s.last_ok == 0.0 {
        return true;
    }
    let wait = if s.more {
        MORE_S
    } else if last_seen > s.last_ok {
        every(s.active_s, ACTIVE_S)
    } else {
        every(s.idle_s, IDLE_S)
    };
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
    // tick·flush가 본 뒤에 공유를 껐을 수 있다. 꺼졌으면 아무 요청도 없이, 상태도 그대로 둔다.
    if !wanted() {
        return Ok(0);
    }
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
    let mut cursor = s.synced_through.clone();
    let result = server::ensure_device().and_then(|token| -> Result<Option<(usize, bool)>, ApiError> {
        // M1은 공유가 켜져 있을 때만 보내므로 share=false 본문은 만들지 않는다(M2 로그인 때 합계 셀).
        let (upload, next, more) = build(state_now, true, &s.synced_through, now as i64);
        cursor = next;
        // config·기기 발급에 최대 30초가 걸린다. 그사이 공유를 껐으면 보내지 않는다.
        if !wanted() {
            return Ok(None);
        }
        if !upload.hours.is_empty() {
            server::put_usage(&token, &upload, timeout)?;
        }
        Ok(Some((upload.hours.len(), more)))
    });
    let out = match result {
        Ok(None) => return Ok(0),
        Ok(Some((sent, more))) => {
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
                    // 400은 같은 본문을 다시 보내도 또 막힌다. 그 묶음의 지난 칸은 건너뛴다.
                    // ponytail: 시계가 서버보다 1시간 넘게 빠르면 지금 칸이 계속 400인데, 클라이언트는
                    // 그걸 알 수 없다. 그동안 지난 칸도 건너뛴다. 서버 Date 헤더로 시계를 맞추면 풀린다.
                    if matches!(e, ApiError::Status(400, _)) {
                        s.synced_through = cursor;
                    }
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
        s.more = true;
        assert!(!due(&s, 1010.0, 0.0) && due(&s, 1030.0, 0.0), "밀린 칸도 30초 간격: 서버는 기기당 분당 2회");
        s.more = false;
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
        let now = crate::watch::now_secs();
        share_on_since(now - 7200.0);
        let done = hour_key(now - 3600.0);
        write_hours(&[json!({"h": done, "r": book("claude-opus-5", 6)})]);
        let state_now = json!({"hour": {"h": hour_key(now), "r": book("claude-opus-5", 7)}});
        assert_eq!(run_once(&state_now, crate::server::NORMAL), Ok(2));
        let reqs: Vec<String> = seen.try_iter().collect();
        assert!(reqs[0].starts_with("GET /v1/config") && reqs[1].starts_with("POST /v1/devices") && reqs[2].starts_with("PUT /v1/usage"), "{reqs:?}");
        assert!(reqs[2].contains("Bearer tmd_x") && reqs[2].contains("\"output_tokens\":6") && reqs[2].contains("\"output_tokens\":7"), "{}", reqs[2]);
        assert!(load().last_ok > 0.0);
        assert_eq!(load().synced_through, done, "지난 칸까지 커서가 간다");
    }

    const CONFIG: &str = r#"{"relay":"","invite":"","active_s":60,"idle_s":900,"live":false}"#;

    fn write_hours(lines: &[Value]) {
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("hours.jsonl"), lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    }

    fn share_on_since(t: f64) {
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("toggle.json"), json!({"share": true, "share_since": t.floor()}).to_string()).unwrap();
    }

    fn sent(up: &UsageUpload) -> Vec<u64> {
        up.hours.iter().map(|h| h.cells[0].output_tokens).collect()
    }

    #[test]
    fn only_hours_that_end_after_sharing_turned_on_are_sent() {
        let (_g, _tmp) = crate::test_home("sync-since");
        let now = crate::watch::now_secs();
        let (old, mid, new) = (hour_key(now - 5.0 * 3600.0), hour_key(now - 3.0 * 3600.0), hour_key(now - 3600.0));
        write_hours(&[
            json!({"h": old, "r": book("claude-opus-5", 1)}),
            json!({"h": mid, "r": book("claude-opus-5", 2)}),
            json!({"h": new, "r": book("claude-opus-5", 3)}),
        ]);
        let state_now = json!({"hour": {"h": hour_key(now), "r": book("claude-opus-5", 4)}});
        share_on_since(now - 4.0 * 3600.0);
        let (up, cursor, _) = build(&state_now, true, "", now as i64);
        assert_eq!((sent(&up), cursor.as_str()), (vec![2, 3, 4], new.as_str()), "켜기 전에 끝난 칸은 빠진다");
        share_on_since(now - 2.0 * 3600.0);
        assert_eq!(sent(&build(&state_now, true, "", now as i64).0), [3, 4], "껐다 켜면 그 사이 칸도 안 간다");
        share_on_since(now);
        let (up, cursor, _) = build(&state_now, true, "", now as i64);
        assert_eq!((sent(&up), cursor.as_str()), (vec![4], new.as_str()), "지금 칸은 가고, 커서는 건너뛴 칸을 지나간다");
    }

    #[test]
    fn hours_the_server_would_reject_are_dropped_and_a_repeated_t_keeps_the_last() {
        let (_g, _tmp) = crate::test_home("sync-reject");
        let now = crate::watch::now_secs();
        let (a, b) = (hour_key(now - 2.0 * 3600.0), hour_key(now - 3600.0));
        let mut crowd = serde_json::Map::new();
        for i in 0..=tokenmeter_protocol::MAX_CELLS {
            crowd.extend(book(&format!("m{i}"), 1).as_object().unwrap().clone());
        }
        write_hours(&[
            json!({"h": a, "r": book("claude-opus-5", 1)}),
            json!({"h": hour_key(now + 3.0 * 3600.0), "r": book("claude-opus-5", 2)}),
            json!({"h": b, "r": crowd}),
            json!({"h": a, "r": book("claude-opus-5", 3)}),
            json!({"h": b, "r": book("claude-opus-5", 4)}),
        ]);
        // 서쪽으로 옮기면 지금 칸의 키가 지난 칸과 같을 수 있다.
        let state_now = json!({"hour": {"h": b, "r": book("claude-opus-5", 5)}});
        let (up, _, more) = build(&state_now, true, "", now as i64);
        assert_eq!(validate(&up, now as i64), Ok(()));
        assert_eq!((sent(&up), more), (vec![3, 5], false), "미래 t·셀 200개 초과는 빠지고 같은 t는 뒤의 것만");
    }

    #[test]
    fn a_long_backlog_stops_before_the_body_limit() {
        let (_g, _tmp) = crate::test_home("sync-size");
        let now = crate::watch::now_secs();
        let mut wide = serde_json::Map::new();
        for i in 0..150 {
            wide.extend(book(&format!("m{i}"), 1).as_object().unwrap().clone());
        }
        write_hours(&(1..=40).rev().map(|k| json!({"h": hour_key(now - k as f64 * 3600.0), "r": wide})).collect::<Vec<_>>());
        let (up, cursor, more) = build(&json!({}), true, "", now as i64);
        let body = serde_json::to_string(&up).unwrap().len();
        assert!(more && body <= 200_000 + 100, "{body} bytes, {} hours", up.hours.len());
        assert_eq!(cursor, hour_key(up.hours.last().unwrap().t as f64), "커서는 담은 마지막 칸까지");
    }

    #[test]
    fn a_rejected_batch_is_skipped_instead_of_retried_forever() {
        let (_g, _tmp) = crate::test_home("sync-400");
        forget();
        let (url, seen) = crate::server::fake::serve(vec![(200, CONFIG), (400, r#"{"error":"invalid","message":"bad t"}"#)]);
        crate::server::fake::use_server(&url);
        fs::write(data_dir().join("device.json"), r#"{"device_id":"d1","token":"tmd_x"}"#).unwrap();
        let now = crate::watch::now_secs();
        share_on_since(now - 7200.0);
        let done = hour_key(now - 3600.0);
        write_hours(&[json!({"h": done, "r": book("claude-opus-5", 6)})]);
        assert_eq!(run_once(&json!({}), crate::server::NORMAL), Err(ApiError::Status(400, "invalid".into())));
        let s = load();
        assert_eq!((s.synced_through.as_str(), s.fails), (done.as_str(), 1), "거부된 묶음은 건너뛰고 실패는 센다");
        assert_eq!(seen.try_iter().filter(|r| r.starts_with("PUT")).count(), 1);
        assert_eq!(run_once(&json!({}), crate::server::NORMAL), Ok(0), "같은 본문을 다시 보내지 않는다");
    }

    #[test]
    fn run_once_sends_nothing_once_sharing_is_off() {
        let (_g, _tmp) = crate::test_home("sync-off");
        forget();
        let (url, seen) = crate::server::fake::serve(vec![(200, CONFIG)]);
        crate::server::fake::use_server(&url);
        crate::share::set(false);
        let now = crate::watch::now_secs();
        let state_now = json!({"hour": {"h": hour_key(now), "r": book("claude-opus-5", 7)}});
        assert_eq!(run_once(&state_now, crate::server::NORMAL), Ok(0));
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err(), "요청이 하나도 없어야 한다");
        assert!(!data_dir().join("league-sync.json").exists() && !data_dir().join("device.json").exists());
    }

    #[test]
    fn flush_without_a_device_token_sends_nothing() {
        let (_g, _tmp) = crate::test_home("sync-flush");
        forget();
        let (url, seen) = crate::server::fake::serve(vec![(201, r#"{"device_id":"d1","token":"tmd_x"}"#), (204, "")]);
        crate::server::fake::use_server(&url);
        crate::share::set(true);
        let now = crate::watch::now_secs();
        flush(&json!({"total": {"last_seen": now}, "hour": {"h": hour_key(now), "r": book("claude-opus-5", 7)}}));
        assert!(seen.recv_timeout(Duration::from_millis(300)).is_err(), "종료 때는 기기 발급을 하지 않는다");
    }

    #[test]
    fn save_does_not_bring_back_a_purged_data_dir() {
        let (_g, _tmp) = crate::test_home("sync-save");
        save(&SyncState { last_ok: 1.0, ..SyncState::default() });
        assert_eq!(load().last_ok, 1.0);
        assert!(!path().with_extension("tmp").exists(), "임시 파일은 남지 않는다");
        fs::remove_dir_all(data_dir()).unwrap();
        save(&SyncState::default());
        assert!(!data_dir().exists(), "uninstall --purge 뒤 데몬의 마지막 저장이 디렉터리를 되살리지 않는다");
    }
}
