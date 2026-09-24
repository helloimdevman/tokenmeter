//! 익명 사용 통계 동의. toggle.json의 `share`에 둔다. 값이 없으면 묻지 않은 것(꺼짐)이다.

use crate::cli::{load_toggle, save_toggle};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};

const PROMPT: &str = "\n  익명 사용 통계를 보낼까요? 도구·모델·경로별 시간당 토큰 수와 추정 비용만 보냅니다.\n  프롬프트·코드·파일 경로·프로젝트명은 보내지 않습니다. 미리 보기: tokenmeter share preview\n  보내기 [Y/n] ";
const OFF_HINT: &str = "익명 사용 통계는 꺼져 있습니다 · 켜기: tokenmeter share on";

pub fn answer() -> Option<bool> {
    load_toggle().get("share").and_then(Value::as_bool)
}

pub fn on() -> bool {
    answer() == Some(true)
}

pub fn set(on: bool) {
    let mut toggle = load_toggle();
    toggle["share"] = json!(on);
    save_toggle(&toggle);
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
    if on() {
        "켜짐 · 보낼 내용: tokenmeter share preview".into()
    } else {
        "꺼짐 — tokenmeter share on".into()
    }
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
}
