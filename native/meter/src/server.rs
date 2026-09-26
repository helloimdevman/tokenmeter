//! Token League 서버 클라이언트. `settings.league.server`가 비어 있으면 네트워크를 쓰지 않는다.

use crate::watch::setting_str;
use crate::VERSION;
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;
use tokenmeter_hook::data_dir;
use tokenmeter_protocol::{DeviceRequest, DeviceResponse, ErrorBody, ServerConfig, UsageUpload};

pub const NORMAL: Duration = Duration::from_secs(15);

#[derive(Debug, PartialEq)]
pub enum ApiError {
    /// 서버 설정이 비어 있다.
    Off,
    /// 서버에 닿지 못했다.
    Offline,
    /// 426: 서버가 이 클라이언트 버전을 더 받지 않는다.
    Upgrade,
    /// 그 밖의 HTTP 오류와 서버의 오류 코드.
    Status(u16, String),
}

pub fn base() -> String {
    setting_str(&["settings", "league", "server"]).trim_end_matches('/').to_string()
}

pub(crate) fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(NORMAL)
            .user_agent(&format!("tokenmeter/{VERSION}"))
            .build()
    })
}

fn call(method: &str, path: &str, token: Option<&str>, body: Option<Value>, timeout: Duration) -> Result<Vec<u8>, ApiError> {
    let base = base();
    if base.is_empty() {
        return Err(ApiError::Off);
    }
    let mut req = agent().request(method, &format!("{base}{path}")).timeout(timeout);
    if let Some(token) = token {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let result = match body {
        Some(body) => req.send_json(body),
        None => req.call(),
    };
    match result {
        Ok(resp) => {
            let mut buf = Vec::new();
            let _ = resp.into_reader().take(1 << 20).read_to_end(&mut buf);
            Ok(buf)
        }
        Err(ureq::Error::Status(426, _)) => Err(ApiError::Upgrade),
        Err(ureq::Error::Status(code, resp)) => Err(ApiError::Status(
            code,
            resp.into_json::<ErrorBody>().map(|b| b.error).unwrap_or_default(),
        )),
        Err(_) => Err(ApiError::Offline),
    }
}

fn device_path() -> PathBuf {
    data_dir().join("device.json")
}

pub fn device_token() -> Option<String> {
    let text = fs::read_to_string(device_path()).ok()?;
    let device: DeviceResponse = serde_json::from_str(&text).ok()?;
    Some(device.token).filter(|t| !t.is_empty())
}

fn save_device(device: &DeviceResponse) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::create_dir_all(data_dir())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(device_path())?;
    file.write_all(serde_json::to_string(device)?.as_bytes())
}

pub fn forget_device() {
    let _ = fs::remove_file(device_path());
}

fn platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH).replace('_', "-")
}

/// 이 설치본의 토큰. 처음이면 서버에서 발급받아 device.json(0600)에 둔다.
pub fn ensure_device() -> Result<String, ApiError> {
    if let Some(token) = device_token() {
        return Ok(token);
    }
    let body = serde_json::to_value(DeviceRequest { platform: platform(), version: VERSION.into() })
        .map_err(|_| ApiError::Offline)?;
    let raw = call("POST", "/v1/devices", None, Some(body), NORMAL)?;
    let device: DeviceResponse =
        serde_json::from_slice(&raw).map_err(|_| ApiError::Status(200, "bad_response".into()))?;
    save_device(&device).map_err(|_| ApiError::Offline)?;
    Ok(device.token)
}

pub fn config() -> Result<ServerConfig, ApiError> {
    let raw = call("GET", "/v1/config", None, None, NORMAL)?;
    serde_json::from_slice(&raw).map_err(|_| ApiError::Status(200, "bad_response".into()))
}

pub fn put_usage(token: &str, upload: &UsageUpload, timeout: Duration) -> Result<(), ApiError> {
    let body = serde_json::to_value(upload).map_err(|_| ApiError::Offline)?;
    call("PUT", "/v1/usage", Some(token), Some(body), timeout).map(|_| ())
}

pub fn delete_account(token: &str) -> Result<(), ApiError> {
    call("DELETE", "/v1/account", Some(token), None, NORMAL).map(|_| ())
}

/// 기기 토큰으로 부르는 JSON API(리그). 204처럼 본문이 없으면 `()`로 받는다.
pub fn json<T: serde::de::DeserializeOwned>(method: &str, path: &str, token: &str, body: Option<Value>) -> Result<T, ApiError> {
    let raw = call(method, path, Some(token), body, NORMAL)?;
    let raw = if raw.is_empty() { b"null".to_vec() } else { raw };
    serde_json::from_slice(&raw).map_err(|_| ApiError::Status(200, "bad_response".into()))
}

#[cfg(test)]
pub(crate) mod fake {
    //! 연결마다 요청 하나를 받는 테스트용 HTTP 서버.
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc::{channel, Receiver};

    /// 준비한 (상태, 본문) 응답을 순서대로 돌려준다. 받은 요청은 "요청줄+헤더\n본문"으로 넘겨준다.
    pub fn serve(replies: Vec<(u16, &'static str)>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            for (status, body) in replies {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    head.push_str(&line);
                }
                let mut buf = vec![0u8; len];
                let _ = reader.read_exact(&mut buf);
                let _ = tx.send(format!("{head}\n{}", String::from_utf8_lossy(&buf)));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (url, rx)
    }

    /// `test_home` 안에서 `settings.league.server`를 `url`로 바꾼다. 빈 문자열이면 서버 없음.
    pub fn use_server(url: &str) {
        let dir = std::path::PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap()).join("tokenmeter");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("services.yaml"), format!("settings:\n  league:\n    server: \"{url}\"\n")).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{serve, use_server};
    use super::*;

    #[test]
    fn device_is_registered_once_and_saved_privately() {
        let (_g, _tmp) = crate::test_home("server-device");
        let (url, seen) = serve(vec![(201, r#"{"device_id":"d1","token":"tmd_abc"}"#)]);
        use_server(&url);
        assert_eq!(ensure_device(), Ok("tmd_abc".into()));
        let req = seen.recv().unwrap();
        assert!(req.starts_with("POST /v1/devices"), "{req}");
        assert!(req.to_lowercase().contains(&format!("user-agent: tokenmeter/{VERSION}")), "{req}");
        assert_eq!(ensure_device(), Ok("tmd_abc".into()), "두 번째는 device.json을 쓴다");
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(data_dir().join("device.json")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn errors_map_to_off_upgrade_and_status() {
        let (_g, _tmp) = crate::test_home("server-errors");
        use_server("");
        assert_eq!(config(), Err(ApiError::Off));
        let (url, seen) = serve(vec![(426, "{}"), (400, r#"{"error":"invalid","message":"bad t"}"#)]);
        use_server(&url);
        let up = UsageUpload { v: 1, share: true, endpoint_id: None, hours: vec![] };
        assert_eq!(put_usage("tok", &up, NORMAL), Err(ApiError::Upgrade));
        assert!(seen.recv().unwrap().contains("Bearer tok"));
        assert_eq!(put_usage("tok", &up, NORMAL), Err(ApiError::Status(400, "invalid".into())));
    }
}
