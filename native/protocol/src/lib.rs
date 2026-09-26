//! TokenMeter 클라이언트와 Token League 서버가 주고받는 것.
//! `docs/protocol/*.schema.json`은 이 타입에서 생성한다(아래 테스트).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const VERSION: u32 = 1;
pub const MAX_HOURS: usize = 48;
pub const MAX_CELLS: usize = 200;
pub const MAX_COUNT: u64 = 10_000_000_000_000;
pub const PAST_SECS: i64 = 30 * 86_400;
pub const FUTURE_SECS: i64 = 3_600;
/// 실제 시간대 차이는 모두 15분의 배수라, 로컬 정각도 900초의 배수다.
pub const HOUR_STEP: i64 = 900;
/// 공유가 꺼진 사용자의 합계 셀은 네 라벨이 모두 이 값이다.
pub const ALL: &str = "*";
/// 방 안 실시간 연결의 ALPN.
pub const LIVE_ALPN: &[u8] = b"tokenleague/live/1";
/// 실시간 한 줄의 최대 바이트('\n' 포함). 넘으면 받는 쪽이 연결을 끊는다.
pub const MAX_LINE: usize = 1024;
/// 한 사용자가 들어가 있을 수 있는 방 수.
pub const MAX_ROOMS: i64 = 8;
/// 한 방의 최대 인원.
pub const MAX_MEMBERS: i64 = 20;

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub relay: String,
    pub invite: String,
    pub active_s: u64,
    pub idle_s: u64,
    pub live: bool,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct DeviceRequest {
    pub platform: String,
    pub version: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct DeviceResponse {
    pub device_id: String,
    pub token: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct UsageUpload {
    pub v: u32,
    pub share: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_id: Option<String>,
    pub hours: Vec<HourCells>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct HourCells {
    /// 로컬 시각 정각의 유닉스 초.
    pub t: i64,
    pub cells: Vec<Cell>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Default)]
pub struct Cell {
    pub client: String,
    pub route: String,
    pub plan: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub calls: u64,
    pub cost_usd: f64,
    pub api_out: u64,
    pub api_ms: u64,
    pub gap_out: u64,
    pub gap_ms: u64,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct ErrorBody {
    pub error: String,
    pub message: String,
}

/// `POST /v1/auth/github`. GitHub 토큰은 서버가 확인한 뒤 바로 폐기한다.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct AuthRequest {
    pub github_token: String,
    /// 이 기기의 iroh EndpointId(소문자 16진수 64자).
    pub endpoint_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct AuthResponse {
    pub user_id: i64,
    pub login: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct RoomList {
    pub rooms: Vec<Room>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct Room {
    /// URL에 쓸 수 있는 12자(`[A-Za-z0-9_-]`).
    pub id: String,
    pub host_id: i64,
    /// 들어온 순서.
    pub members: Vec<Member>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
pub struct Member {
    pub user_id: i64,
    pub login: String,
    /// 로그인한 기기마다 하나.
    pub endpoints: Vec<String>,
}

/// 멤버끼리 단방향 스트림에 보내는 한 줄(JSON + '\n'). 이름은 보내지 않는다:
/// 받는 쪽이 연결 상대의 EndpointId를 멤버 목록에 맞춘다. 모르는 필드는 무시한다.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Default)]
pub struct LiveLine {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tps: Option<f64>,
}

pub fn endpoint_ok(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn room_id_ok(id: &str) -> bool {
    id.len() == 12 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

impl Cell {
    pub fn is_total(&self) -> bool {
        [&self.client, &self.route, &self.plan, &self.model]
            .iter()
            .all(|s| s.as_str() == ALL)
    }

    fn counts(&self) -> [u64; 9] {
        [
            self.input_tokens,
            self.output_tokens,
            self.cache_read,
            self.cache_write,
            self.calls,
            self.api_out,
            self.api_ms,
            self.gap_out,
            self.gap_ms,
        ]
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Label {
    Client,
    Route,
    Plan,
}

impl Label {
    fn rule(self) -> (usize, &'static [u8]) {
        match self {
            Label::Client => (32, b"-"),
            Label::Route => (64, b".-"),
            Label::Plan => (32, b"_-"),
        }
    }

    pub fn ok(self, s: &str) -> bool {
        let (max, extra) = self.rule();
        !s.is_empty()
            && s.len() <= max
            && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || extra.contains(&b))
    }

    /// 어떤 문자열이든 `ok`를 통과하게 만든다: 소문자로, 나머지 문자는 '-', 비면 "unknown".
    pub fn clean(self, raw: &str) -> String {
        let (max, extra) = self.rule();
        let s: String = raw
            .to_lowercase()
            .bytes()
            .map(|b| {
                if b.is_ascii_lowercase() || b.is_ascii_digit() || extra.contains(&b) {
                    b as char
                } else {
                    '-'
                }
            })
            .take(max)
            .collect();
        if s.is_empty() {
            "unknown".into()
        } else {
            s
        }
    }
}

fn model_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 96 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._:@+-".contains(&b))
}

/// 서버와 같은 규칙으로 업로드를 검사한다. `now`는 유닉스 초.
pub fn validate(up: &UsageUpload, now: i64) -> Result<(), String> {
    if up.v != VERSION {
        return Err(format!("unsupported v {}", up.v));
    }
    if up.hours.len() > MAX_HOURS {
        return Err(format!("more than {MAX_HOURS} hours"));
    }
    if up.endpoint_id.as_deref().is_some_and(|id| !endpoint_ok(id)) {
        return Err("bad endpoint_id".into());
    }
    let mut hours = HashSet::new();
    for hour in &up.hours {
        if hour.t % HOUR_STEP != 0 || hour.t < now - PAST_SECS || hour.t > now + FUTURE_SECS {
            return Err(format!("bad t {}", hour.t));
        }
        if !hours.insert(hour.t) {
            return Err(format!("duplicate t {}", hour.t));
        }
        if hour.cells.len() > MAX_CELLS {
            return Err(format!("more than {MAX_CELLS} cells"));
        }
        let mut keys = HashSet::new();
        for c in &hour.cells {
            if !keys.insert((&c.client, &c.route, &c.plan, &c.model)) {
                return Err("duplicate cell".into());
            }
            if c.is_total() {
                // 합계 셀은 공유 여부와 상관없이 된다.
            } else if !up.share {
                return Err("detail cell with share off".into());
            } else if !(Label::Client.ok(&c.client)
                && Label::Route.ok(&c.route)
                && Label::Plan.ok(&c.plan)
                && model_ok(&c.model))
            {
                return Err("bad label".into());
            }
            let cost_ok = c.cost_usd.is_finite() && (0.0..=MAX_COUNT as f64).contains(&c.cost_usd);
            if c.counts().iter().any(|&n| n > MAX_COUNT) || !cost_ok {
                return Err("bad number".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_000_100;

    fn hour(k: i64) -> i64 {
        (NOW / 3600 - k) * 3600
    }

    fn cell(model: &str) -> Cell {
        Cell {
            client: "claude-code".into(),
            route: "api.anthropic.com".into(),
            plan: "subscription".into(),
            model: model.into(),
            input_tokens: 10,
            output_tokens: 20,
            calls: 1,
            cost_usd: 0.5,
            ..Cell::default()
        }
    }

    fn total() -> Cell {
        Cell {
            client: ALL.into(),
            route: ALL.into(),
            plan: ALL.into(),
            model: ALL.into(),
            output_tokens: 5,
            ..Cell::default()
        }
    }

    fn upload(share: bool, cells: Vec<Cell>) -> UsageUpload {
        UsageUpload { v: VERSION, share, endpoint_id: None, hours: vec![HourCells { t: hour(1), cells }] }
    }

    #[test]
    fn accepts_details_with_share_and_totals_without() {
        assert_eq!(validate(&upload(true, vec![cell("claude-opus-5"), cell("other")]), NOW), Ok(()));
        assert_eq!(validate(&upload(false, vec![total()]), NOW), Ok(()));
    }

    #[test]
    fn rejects_details_when_share_is_off() {
        assert!(validate(&upload(false, vec![cell("claude-opus-5")]), NOW).is_err());
    }

    #[test]
    fn rejects_bad_hours() {
        let mut up = upload(true, vec![cell("x")]);
        up.hours[0].t += 60;
        assert!(validate(&up, NOW).is_err(), "15분 경계가 아님");
        up.hours[0].t = hour(24 * 31);
        assert!(validate(&up, NOW).is_err(), "30일보다 오래됨");
        up.hours[0].t = hour(-2);
        assert!(validate(&up, NOW).is_err(), "1시간보다 앞섬");
        let mut many = upload(true, vec![]);
        many.hours = (0..=MAX_HOURS as i64).map(|k| HourCells { t: hour(k), cells: vec![] }).collect();
        assert!(validate(&many, NOW).is_err());
        let mut dup = upload(true, vec![]);
        dup.hours = vec![HourCells { t: hour(1), cells: vec![] }, HourCells { t: hour(1), cells: vec![] }];
        assert!(validate(&dup, NOW).is_err());
    }

    #[test]
    fn rejects_bad_labels_numbers_and_duplicates() {
        for bad in ["", "Claude Code", "../etc", "a*b"] {
            let mut c = cell("claude-opus-5");
            c.client = bad.into();
            assert!(validate(&upload(true, vec![c]), NOW).is_err(), "{bad:?}");
        }
        let mut c = cell("x");
        c.model = "arn:aws:bedrock:us-east-1:1/abc".into();
        assert!(validate(&upload(true, vec![c]), NOW).is_err());
        let mut c = cell("x");
        c.output_tokens = MAX_COUNT + 1;
        assert!(validate(&upload(true, vec![c]), NOW).is_err());
        let mut c = cell("x");
        c.cost_usd = f64::NAN;
        assert!(validate(&upload(true, vec![c]), NOW).is_err());
        assert!(validate(&upload(true, vec![cell("x"), cell("x")]), NOW).is_err());
        let mut up = upload(true, vec![cell("x")]);
        up.endpoint_id = Some("zz".into());
        assert!(validate(&up, NOW).is_err());
    }

    #[test]
    fn clean_makes_any_label_pass() {
        for (label, raw) in [(Label::Client, "My Agent_2"), (Label::Route, "LLM.Corp/v1"), (Label::Plan, ""), (Label::Plan, "팀 요금제")] {
            let s = label.clean(raw);
            assert!(label.ok(&s), "{label:?} {raw:?} -> {s:?}");
        }
        assert_eq!(Label::Client.clean(""), "unknown");
    }

    #[test]
    fn ids_follow_the_rules() {
        assert!(endpoint_ok(&"ae".repeat(32)));
        for bad in ["", "AE".repeat(32).as_str(), "g".repeat(64).as_str(), "a".repeat(63).as_str()] {
            assert!(!endpoint_ok(bad), "{bad:?}");
        }
        assert!(room_id_ok("aZ0_-aZ0_-aZ"));
        for bad in ["", "short", "aZ0_-aZ0_-aZ0", "aZ0_-aZ0_-a/", "방방방방"] {
            assert!(!room_id_ok(bad), "{bad:?}");
        }
    }

    #[test]
    fn live_lines_ignore_what_they_do_not_know() {
        let line: LiveLine = serde_json::from_str(r#"{"tps": 12.5, "later": 1}"#).unwrap();
        assert_eq!(line.tps, Some(12.5));
        assert_eq!(serde_json::to_string(&LiveLine::default()).unwrap(), "{}");
    }

    fn pretty<T: serde::Serialize>(v: &T) -> String {
        serde_json::to_string_pretty(v).unwrap() + "\n"
    }

    #[test]
    fn schemas_match_docs() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/protocol");
        let schemas = [
            ("config", pretty(&schemars::schema_for!(ServerConfig))),
            ("device-request", pretty(&schemars::schema_for!(DeviceRequest))),
            ("device-response", pretty(&schemars::schema_for!(DeviceResponse))),
            ("usage-upload", pretty(&schemars::schema_for!(UsageUpload))),
            ("error", pretty(&schemars::schema_for!(ErrorBody))),
            ("auth-request", pretty(&schemars::schema_for!(AuthRequest))),
            ("auth-response", pretty(&schemars::schema_for!(AuthResponse))),
            ("room", pretty(&schemars::schema_for!(Room))),
            ("rooms", pretty(&schemars::schema_for!(RoomList))),
            ("live-line", pretty(&schemars::schema_for!(LiveLine))),
        ];
        for (name, text) in schemas {
            let path = dir.join(format!("{name}.schema.json"));
            if std::env::var("UPDATE_SCHEMAS").is_ok() {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &text).unwrap();
                continue;
            }
            let saved = std::fs::read_to_string(&path).unwrap_or_default();
            assert_eq!(saved, text, "{} 가 낡았다: UPDATE_SCHEMAS=1 cargo test -p tokenmeter-protocol", path.display());
        }
    }
}
