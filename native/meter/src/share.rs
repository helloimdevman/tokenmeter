//! 익명 사용 통계 동의. toggle.json의 `share`에 둔다. 값이 없으면 묻지 않은 것(꺼짐)이다.

use crate::cli::{load_toggle, save_toggle};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

const PROMPT: &str = "\n  익명 사용 통계를 보낼까요? 켠 때부터 도구·경로 라벨·요금제·모델 계열별 시간당 토큰 수,\n  요청 수, 추정 비용, 시간 합과 플랫폼·버전을 보냅니다.\n  프롬프트·코드·파일 경로·프로젝트명은 보내지 않습니다. 미리 보기: tokenmeter share preview\n  보내기 [Y/n] ";
const OFF_HINT: &str = "익명 사용 통계는 꺼져 있습니다 · 켜기: tokenmeter share on";

pub fn answer() -> Option<bool> {
    load_toggle().get("share").and_then(Value::as_bool)
}

pub fn on() -> bool {
    answer() == Some(true)
}

pub fn set(on: bool) {
    let mut toggle = load_toggle();
    // 꺼져 있다가 켤 때만 시각을 적는다. 그 전에 끝난 칸은 보내지 않는다(sync::build).
    // 정수 초로 둔다 — serde_json 기본 파서는 소수 끝자리를 정확히 되돌리지 못한다.
    if on && toggle.get("share") != Some(&json!(true)) {
        toggle["share_since"] = json!(crate::watch::now_secs().floor());
    }
    toggle["share"] = json!(on);
    save_toggle(&toggle);
}

/// 공유를 마지막으로 켠 유닉스 초. 없으면 0.
pub fn since() -> f64 {
    load_toggle().get("share_since").and_then(Value::as_f64).unwrap_or(0.0)
}

pub fn parse_answer(line: &str) -> Option<bool> {
    match line.trim().to_lowercase().as_str() {
        "" | "y" | "yes" | "예" | "네" | "ㅇ" => Some(true),
        "n" | "no" | "아니오" | "아니요" | "ㄴ" => Some(false),
        _ => None,
    }
}

/// `/dev/tty`로 한 번만 묻는다(`curl | sh`에서는 표준 입력이 스크립트다).
/// 터미널이 없으면(CI 등) 답을 적지 않고 꺼진 채로 둔다.
pub fn ask_once() -> Vec<String> {
    if answer().is_some() {
        return Vec::new();
    }
    if cfg!(test) || std::env::var("TOKENMETER_NO_PROMPT").is_ok() {
        return vec![OFF_HINT.into()];
    }
    let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        return vec![OFF_HINT.into()];
    };
    let Ok(mut out) = tty.try_clone() else {
        return vec![OFF_HINT.into()];
    };
    let mut input = BufReader::new(tty);
    for _ in 0..3 {
        let _ = write!(out, "{PROMPT}");
        let _ = out.flush();
        let mut line = String::new();
        if input.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if let Some(yes) = parse_answer(&line) {
            set(yes);
            let msg = if yes { "익명 사용 통계를 켰습니다 · 끄기: tokenmeter share off" } else { OFF_HINT };
            return vec![msg.into()];
        }
    }
    vec![OFF_HINT.into()]
}

pub fn caption() -> String {
    if !on() {
        return "꺼짐 — tokenmeter share on".into();
    }
    let s = crate::sync::load();
    if s.upgrade_for == crate::VERSION {
        return "켜짐 · 서버가 새 버전을 요구합니다 — tokenmeter update now".into();
    }
    if s.fails > 0 {
        return "켜짐 · 전송 재시도 중 — tokenmeter share preview".into();
    }
    if s.last_ok > 0.0 {
        let c = crate::history::civil_of(s.last_ok);
        return format!("켜짐 · 마지막 전송 {:02}/{:02} {:02}:{:02}", c.month, c.day, c.hour, c.minute);
    }
    "켜짐 · 아직 보내지 않음".into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenmeter_hook::data_dir;

    #[test]
    fn answers_round_trip_and_keep_other_toggles() {
        let (_g, _tmp) = crate::test_home("share");
        assert_eq!(answer(), None);
        assert!(!on() && caption().starts_with("꺼짐"));
        assert_eq!(ask_once(), [OFF_HINT], "묻지 못하면 켜는 명령만 알린다");
        assert_eq!(answer(), None, "묻지 못했으면 답을 적지 않는다");
        std::fs::create_dir_all(data_dir()).unwrap();
        std::fs::write(data_dir().join("toggle.json"), r#"{"auto_update": true}"#).unwrap();
        set(true);
        assert!(on() && caption().starts_with("켜짐"));
        assert_eq!(load_toggle()["auto_update"], json!(true), "자동 업데이트 값은 그대로");
        set(false);
        assert_eq!(answer(), Some(false));
        assert!(ask_once().is_empty(), "이미 답했으면 다시 묻지 않는다");
        for (line, want) in [("\n", Some(true)), ("Y", Some(true)), ("예", Some(true)), ("n", Some(false)), ("아니요", Some(false)), ("maybe", None)] {
            assert_eq!(parse_answer(line), want, "{line:?}");
        }
    }

    #[test]
    fn since_is_written_only_when_sharing_turns_on() {
        let (_g, _tmp) = crate::test_home("share-since");
        assert_eq!(since(), 0.0);
        std::fs::create_dir_all(data_dir()).unwrap();
        std::fs::write(data_dir().join("toggle.json"), r#"{"share": true, "share_since": 5}"#).unwrap();
        set(true);
        assert_eq!(since(), 5.0, "이미 켜져 있으면 그대로");
        set(false);
        assert_eq!(since(), 5.0);
        set(true);
        assert!(since() > 5.0, "껐다 켜면 그 시각부터 다시");
    }

    #[test]
    fn caption_shows_retries_and_prompt_names_every_field() {
        let (_g, _tmp) = crate::test_home("share-caption");
        std::fs::create_dir_all(data_dir()).unwrap();
        std::fs::write(data_dir().join("toggle.json"), r#"{"share": true}"#).unwrap();
        std::fs::write(data_dir().join("league-sync.json"), r#"{"last_ok": 100, "fails": 2}"#).unwrap();
        assert!(caption().contains("재시도"), "{}", caption());
        for field in ["요금제", "요청 수", "추정 비용", "시간 합", "플랫폼·버전", "켠 때부터"] {
            assert!(PROMPT.contains(field), "{field}");
        }
        assert!(!PROMPT.contains("만 보냅니다"));
    }
}
