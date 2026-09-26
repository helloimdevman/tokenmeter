//! GitHub Device Flow. 받은 토큰은 리그 서버에 한 번 보내고 버린다(디스크에 쓰지 않는다).

use serde::de::DeserializeOwned;
use serde::Deserialize;

pub const GITHUB: &str = "https://github.com";
/// OAuth 앱 client id는 공개 값이다. 비밀값은 서버에만 있다.
pub const CLIENT_ID: &str = "Ov23li8kfihyKkRGxEob";
const GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Deserialize, Debug)]
pub struct Code {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u64,
}

#[derive(Deserialize)]
struct Poll {
    access_token: Option<String>,
    error: Option<String>,
}

fn post<T: DeserializeOwned>(url: &str, form: &[(&str, &str)]) -> Result<T, String> {
    crate::server::agent()
        .post(url)
        .set("Accept", "application/json")
        .send_form(form)
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())
}

/// 1단계: 사용자 코드와 확인 주소(scope 없음).
pub fn start(base: &str) -> Result<Code, String> {
    post(&format!("{base}/login/device/code"), &[("client_id", CLIENT_ID)])
}

/// 다음 요청까지 기다릴 초. None이면 멈춘다(expired_token, access_denied 등).
pub fn next_wait(wait: u64, error: &str) -> Option<u64> {
    match error {
        "authorization_pending" => Some(wait),
        "slow_down" => Some(wait + 5),
        _ => None,
    }
}

/// 2단계: 사용자가 코드를 넣을 때까지 `interval`초마다 묻는다. 테스트는 `sleep`을 비운다.
pub fn wait_token(base: &str, code: &Code, sleep: impl Fn(u64)) -> Result<String, String> {
    let url = format!("{base}/login/oauth/access_token");
    let mut wait = code.interval;
    loop {
        sleep(wait);
        let poll: Poll = post(&url, &[("client_id", CLIENT_ID), ("device_code", &code.device_code), ("grant_type", GRANT)])?;
        if let Some(token) = poll.access_token.filter(|t| !t.is_empty()) {
            return Ok(token);
        }
        let error = poll.error.unwrap_or_else(|| "no_token".into());
        wait = next_wait(wait, &error).ok_or(error)?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::fake::serve;

    #[test]
    fn polls_until_the_user_enters_the_code() {
        let (url, seen) = serve(vec![
            (200, r#"{"device_code":"dc1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0}"#),
            (200, r#"{"error":"authorization_pending"}"#),
            (200, r#"{"access_token":"gho_x","token_type":"bearer","scope":""}"#),
        ]);
        let code = start(&url).unwrap();
        assert_eq!(code.user_code, "ABCD-1234");
        assert_eq!(wait_token(&url, &code, |_| {}), Ok("gho_x".into()));
        let reqs: Vec<String> = seen.try_iter().collect();
        assert!(reqs[0].starts_with("POST /login/device/code") && reqs[0].contains("client_id=Ov23li8kfihyKkRGxEob"), "{}", reqs[0]);
        assert!(!reqs[0].contains("scope"), "scope 없음");
        assert!(reqs[2].contains("device_code=dc1") && reqs[2].contains("grant_type=urn"), "{}", reqs[2]);
    }

    #[test]
    fn slow_down_adds_five_seconds_and_other_errors_stop() {
        assert_eq!(next_wait(5, "authorization_pending"), Some(5));
        assert_eq!(next_wait(5, "slow_down"), Some(10));
        for stop in ["expired_token", "access_denied", "device_flow_disabled", "no_token"] {
            assert_eq!(next_wait(5, stop), None, "{stop}");
        }
    }
}
