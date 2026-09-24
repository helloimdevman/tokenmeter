//! Token League 방. Firebase 리그는 걷어냈고, 새 방은 다음 릴리스(M2)에서 연다.
//! 그때까지 오버레이와 CLI의 호출부를 그대로 두려고 동작 없는 함수만 남긴다.

use serde_json::Value;
use std::fs;
use tokenmeter_hook::data_dir;

const SOON: &str = "Token League 방은 다음 릴리스에서 열립니다";

pub fn enabled() -> bool {
    false
}
pub fn has_auth() -> bool {
    false
}
pub fn list_rooms() -> Vec<Value> {
    Vec::new()
}
pub fn current_focus() -> String {
    String::new()
}
pub fn caption() -> String {
    "다음 릴리스에서 열림".into()
}
pub fn overlay_step() -> &'static str {
    ""
}
pub fn spawn_login_cli() {}
pub fn invite_link(_rid: &str) -> String {
    String::new()
}
pub fn focus_room(_rid: &str) {}
pub fn room_open() -> bool {
    false
}
pub fn tick(_status: &Value, _tps: f64) {}

fn soon() -> i32 {
    println!("{SOON}");
    0
}
pub fn login() -> i32 {
    soon()
}
pub fn logout() -> i32 {
    soon()
}
pub fn open_room(_rule: &str) -> i32 {
    soon()
}
pub fn join_room(_room: &str) -> i32 {
    soon()
}
pub fn leave(_rid: Option<&str>) -> i32 {
    soon()
}
pub fn close_room(_rid: Option<&str>) -> i32 {
    soon()
}

/// Firebase 시절 파일은 데몬 유휴 종료를 막고 죽은 백엔드를 가리킨다.
/// league-auth.json에 `refresh_token`이 있으면(옛 Google 로그인) 그 파일과 league.json·league-cache.json을 지운다. 한 번 지우면 끝난다.
pub fn cleanup_legacy() {
    let auth = data_dir().join("league-auth.json");
    if fs::read_to_string(&auth).map(|t| t.contains("refresh_token")).unwrap_or(false) {
        for name in ["league-auth.json", "league.json", "league-cache.json"] {
            let _ = fs::remove_file(data_dir().join(name));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rooms_stay_off_until_the_next_release() {
        assert!(!enabled() && !has_auth() && list_rooms().is_empty() && overlay_step().is_empty());
        assert_eq!(join_room("abcd"), 0);
    }

    #[test]
    fn legacy_firebase_files_are_removed() {
        let (_g, _tmp) = crate::test_home("league-legacy");
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("league-auth.json"), r#"{"refresh_token":"x","uid":"u","handle":"h"}"#).unwrap();
        fs::write(data_dir().join("league.json"), r#"{"room_id":"abcd"}"#).unwrap();
        cleanup_legacy();
        assert!(!data_dir().join("league-auth.json").exists() && !data_dir().join("league.json").exists());
    }
}
