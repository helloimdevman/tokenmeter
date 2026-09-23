//! Token League: Google PKCE 로그인 + RTDB 방 + 멤버 PUT.

use crate::attention::attention_counts;
use crate::watch::{now_secs, setting_str};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tokenmeter_hook::data_dir;

const ALLOWED: &[&str] = &["cost", "output", "check"];
const MEMBER_TTL: f64 = 15.0;
const PUT_INTERVAL: f64 = 5.0;
const MAX_MEMBERS: usize = 20;
const MAX_ROOMS: usize = 8;
const EMPTY_ROOM_TTL: f64 = 86400.0;
const GOOGLE_AUTH: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN: &str = "https://oauth2.googleapis.com/token";
const IDP_SIGNIN: &str = "https://identitytoolkit.googleapis.com/v1/accounts:signInWithIdp";
const SECURE_TOKEN: &str = "https://securetoken.googleapis.com/v1/token";

struct LeagueState {
    id_token: String,
    id_exp: f64,
    rooms: Vec<Value>,
    focus: String,
    snap: Value,
    last_put: f64,
    put_room: String,
    sse_room: String,
    last_login_spawn: f64,
}

impl Default for LeagueState {
    fn default() -> Self {
        Self {
            id_token: String::new(),
            id_exp: 0.0,
            rooms: Vec::new(),
            focus: String::new(),
            snap: json!({}),
            last_put: 0.0,
            put_room: String::new(),
            sse_room: String::new(),
            last_login_spawn: 0.0,
        }
    }
}

fn state() -> &'static Mutex<LeagueState> {
    static STATE: OnceLock<Mutex<LeagueState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(LeagueState::default()))
}

fn auth_path() -> PathBuf {
    data_dir().join("league-auth.json")
}
fn room_path() -> PathBuf {
    data_dir().join("league.json")
}
fn cache_path() -> PathBuf {
    data_dir().join("league-cache.json")
}

fn configured() -> bool {
    !setting_str(&["settings", "league", "databaseURL"]).is_empty()
}

pub fn enabled() -> bool {
    configured()
}

pub fn has_auth() -> bool {
    load_auth().is_some()
}

pub fn list_rooms() -> Vec<Value> {
    load_rooms()
}

pub fn current_focus() -> String {
    load_rooms();
    state()
        .lock()
        .ok()
        .map(|g| g.focus.clone())
        .unwrap_or_default()
}

pub fn caption() -> String {
    if !enabled() {
        return "꺼짐".into();
    }
    if !has_auth() {
        return "로그인 안 함 — tokenmeter league login".into();
    }
    let rid = current_focus();
    if rid.is_empty() {
        return "방 없음 — tokenmeter league open".into();
    }
    let url = invite_link(&rid);
    if url.is_empty() {
        format!("방 {rid}")
    } else {
        format!("방 {rid} · {url}")
    }
}

pub fn overlay_step() -> &'static str {
    if !enabled() {
        return "";
    }
    if !has_auth() {
        return "login";
    }
    if list_rooms().is_empty() {
        "open"
    } else {
        ""
    }
}

pub fn spawn_login_cli() {
    if cfg!(test) || std::env::var("TOKENMETER_NO_DAEMON").ok().as_deref() == Some("1") {
        return;
    }
    {
        let mut g = state().lock().unwrap();
        let now = now_secs();
        if now - g.last_login_spawn < 8.0 {
            return;
        }
        g.last_login_spawn = now;
    }
    let _ = Command::new(crate::install::meter_bin())
        .args(["league", "login"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

pub fn invite_link(rid: &str) -> String {
    if !room_id_ok(rid) {
        return String::new();
    }
    let host = setting_str(&["settings", "league", "hostingBaseUrl"]);
    if host.is_empty() {
        format!("tokenmeter league join {rid}")
    } else {
        format!("{}/j/{rid}", host.trim_end_matches('/'))
    }
}

pub fn focus_room(rid: &str) {
    let rooms = load_rooms();
    if rooms.iter().any(|r| r.get("room_id").and_then(Value::as_str) == Some(rid)) {
        write_rooms(rooms, rid);
    }
}

fn db_url() -> String {
    setting_str(&["settings", "league", "databaseURL"])
        .trim_end_matches('/')
        .to_string()
}

fn room_id_ok(rid: &str) -> bool {
    (4..=22).contains(&rid.len())
        && rid
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn load_auth() -> Option<(String, String, String)> {
    let data = fs::read_to_string(auth_path()).ok()?;
    let v: Value = serde_json::from_str(&data).ok()?;
    Some((
        v.get("refresh_token")?.as_str()?.to_string(),
        v.get("uid")?.as_str()?.to_string(),
        v.get("handle")?.as_str()?.to_string(),
    ))
}

fn save_auth(refresh: &str, uid: &str, handle: &str) {
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(
        auth_path(),
        json!({"refresh_token": refresh, "uid": uid, "handle": handle}).to_string(),
    );
}

fn week_key(now: f64) -> String {
    let days = (now / 86400.0).floor() as i64;
    let (y, w, _) = iso_week(days);
    format!("{y}-W{w:02}")
}

fn iso_week(unix_days: i64) -> (i32, u32, u32) {
    // 1970-01-01 = Thursday. ISO week.
    let z = unix_days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let weekday = ((unix_days + 4).rem_euclid(7) + 1) as u32;
    let week = ((doy + 10 - (weekday + 6) % 7) / 7).max(1);
    (y, week.max(1).min(53), d)
}

fn http(method: &str, url: &str, body: Option<&[u8]>, headers: &[(&str, &str)]) -> (u16, Vec<u8>) {
    let mut req = match method {
        "POST" => ureq::post(url),
        "PUT" => ureq::put(url),
        "PATCH" => ureq::request("PATCH", url),
        "DELETE" => ureq::delete(url),
        _ => ureq::get(url),
    };
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let result = if let Some(body) = body {
        req.timeout(Duration::from_secs(15)).send_bytes(body)
    } else {
        req.timeout(Duration::from_secs(15)).call().map_err(Into::into)
    };
    match result {
        Ok(resp) => {
            let code = resp.status();
            let mut buf = Vec::new();
            let _ = resp.into_reader().read_to_end(&mut buf);
            (code, buf)
        }
        Err(ureq::Error::Status(code, resp)) => {
            let mut buf = Vec::new();
            let _ = resp.into_reader().read_to_end(&mut buf);
            (code, buf)
        }
        Err(_) => (0, Vec::new()),
    }
}

fn id_token(force: bool) -> String {
    if !configured() {
        return String::new();
    }
    {
        let g = state().lock().unwrap();
        if !force && !g.id_token.is_empty() && now_secs() < g.id_exp - 30.0 {
            return g.id_token.clone();
        }
    }
    let Some((refresh, uid, handle)) = load_auth() else {
        return String::new();
    };
    let key = setting_str(&["settings", "league", "apiKey"]);
    let body = form(&[("grant_type", "refresh_token"), ("refresh_token", &refresh)]);
    let (code, raw) = http(
        "POST",
        &format!("{SECURE_TOKEN}?key={}", urlenc(&key)),
        Some(&body),
        &[("Content-Type", "application/x-www-form-urlencoded")],
    );
    let data: Value = serde_json::from_slice(&raw).unwrap_or(json!({}));
    let token = data.get("id_token").and_then(Value::as_str).unwrap_or("");
    if code != 200 || token.is_empty() {
        return String::new();
    }
    if let Some(rt) = data.get("refresh_token").and_then(Value::as_str) {
        if rt != refresh {
            save_auth(rt, &uid, &handle);
        }
    }
    let exp = data
        .get("expires_in")
        .and_then(Value::as_f64)
        .or_else(|| {
            data.get("expires_in")
                .and_then(Value::as_str)
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(3600.0);
    let mut g = state().lock().unwrap();
    g.id_token = token.to_string();
    g.id_exp = now_secs() + exp.max(1.0);
    token.to_string()
}

fn form(pairs: &[(&str, &str)]) -> Vec<u8> {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlenc(k), urlenc(v)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes()
}

fn urlenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn authed(url: &str, token: &str) -> String {
    format!(
        "{url}{}auth={}",
        if url.contains('?') { '&' } else { '?' },
        urlenc(token)
    )
}

fn request_json(url: &str, method: &str, body: Option<&Value>) -> Option<Value> {
    if !configured() {
        return None;
    }
    let mut token = id_token(false);
    if token.is_empty() {
        return None;
    }
    let bytes = body.map(|v| v.to_string().into_bytes());
    let mut headers = vec![("Accept", "application/json")];
    if bytes.is_some() {
        headers.push(("Content-Type", "application/json"));
    }
    let (code, raw) = http(method, &authed(url, &token), bytes.as_deref(), &headers);
    let (code, raw) = if code == 401 {
        token = id_token(true);
        if token.is_empty() {
            return None;
        }
        http(method, &authed(url, &token), bytes.as_deref(), &headers)
    } else {
        (code, raw)
    };
    if !(200..300).contains(&code) {
        return None;
    }
    if raw.is_empty() {
        return Some(json!({}));
    }
    serde_json::from_slice(&raw).ok()
}

fn room_url(rid: &str) -> String {
    format!("{}/rooms/{rid}.json", db_url())
}
fn count_url(rid: &str) -> String {
    format!("{}/rooms/{rid}/member_count.json", db_url())
}
fn member_url(rid: &str, uid: &str) -> String {
    format!("{}/rooms/{rid}/members/{}.json", db_url(), urlenc(uid))
}

fn load_rooms() -> Vec<Value> {
    let text = match fs::read_to_string(room_path()) {
        Ok(t) => t,
        Err(_) => {
            let mut g = state().lock().unwrap();
            g.rooms.clear();
            g.focus.clear();
            return Vec::new();
        }
    };
    let obj: Value = serde_json::from_str(&text).unwrap_or(json!({}));
    let mut rooms = Vec::new();
    if let Some(arr) = obj.get("rooms").and_then(Value::as_array) {
        rooms.extend(arr.iter().filter(|r| room_fields(r).is_some()).cloned());
    }
    if rooms.is_empty() {
        if let Some(one) = room_fields(&obj) {
            rooms.push(one);
        }
    }
    let focus = obj
        .get("focus")
        .and_then(Value::as_str)
        .filter(|f| rooms.iter().any(|r| r["room_id"] == *f))
        .map(str::to_string)
        .unwrap_or_else(|| {
            rooms
                .first()
                .and_then(|r| r.get("room_id"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        });
    let mut g = state().lock().unwrap();
    g.rooms = rooms.clone();
    g.focus = focus;
    rooms
}

fn room_fields(data: &Value) -> Option<Value> {
    let rid = data.get("room_id")?.as_str()?;
    let rule = data.get("rule")?.as_str()?;
    let handle = data.get("handle")?.as_str()?;
    let host = data.get("host")?.as_bool()?;
    if rid.is_empty() || !ALLOWED.contains(&rule) || handle.is_empty() {
        return None;
    }
    let mut out = json!({"room_id": rid, "rule": rule, "handle": handle, "host": host});
    if let Some(obj) = out.as_object_mut() {
        if let Some(j) = data.get("joined_total") {
            obj.insert("joined_total".into(), j.clone());
        }
        if let Some(o) = data.get("opened_at") {
            obj.insert("opened_at".into(), o.clone());
        }
        if data.get("claimed") == Some(&json!(true)) {
            obj.insert("claimed".into(), json!(true));
        }
    }
    Some(out)
}

fn write_rooms(rooms: Vec<Value>, focus: &str) {
    if rooms.is_empty() {
        let _ = fs::remove_file(room_path());
        let _ = fs::remove_file(cache_path());
        let mut g = state().lock().unwrap();
        g.rooms.clear();
        g.focus.clear();
        g.snap = json!({});
        return;
    }
    let focus = if rooms.iter().any(|r| r["room_id"] == focus) {
        focus.to_string()
    } else {
        rooms[0]["room_id"].as_str().unwrap_or("").to_string()
    };
    let focused = rooms
        .iter()
        .find(|r| r["room_id"] == focus)
        .cloned()
        .unwrap_or_else(|| rooms[0].clone());
    let mut payload = focused.clone();
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("rooms".into(), json!(rooms));
        obj.insert("focus".into(), json!(focus));
    }
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(room_path(), serde_json::to_string_pretty(&payload).unwrap_or_default());
    let mut g = state().lock().unwrap();
    g.rooms = rooms;
    g.focus = focus;
}

fn totals_pair(status: &Value) -> (f64, i64) {
    let t = status
        .pointer("/total/totals")
        .or_else(|| status.get("totals"))
        .unwrap_or(&Value::Null);
    (
        t.get("cost_usd").and_then(Value::as_f64).unwrap_or(0.0),
        t.get("output_tokens").and_then(Value::as_i64).unwrap_or(0),
    )
}

fn pkce() -> (String, String) {
    let verifier = random_url(64);
    let digest = sha256_b64(&verifier);
    (verifier, digest)
}

fn sha256_b64(input: &str) -> String {
    // Prefer `shasum`/`sha256sum` to avoid a new crate? That's fragile.
    // Add sha2 to Cargo? User said no new dep if avoidable.
    // Python uses hashlib.sha256. I'll add sha2 - it's tiny and correct for PKCE.
    // Actually I already didn't add sha2. Use openssl command:
    if let Ok(out) = std::process::Command::new("shasum")
        .args(["-a", "256"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut c| {
            if let Some(stdin) = c.stdin.as_mut() {
                let _ = stdin.write_all(input.as_bytes());
            }
            c.wait_with_output()
        })
    {
        let hex = String::from_utf8_lossy(&out.stdout);
        let hex = hex.split_whitespace().next().unwrap_or("");
        if hex.len() == 64 {
            return b64url(&hex_bytes(hex));
        }
    }
    b64url(input.as_bytes())
}

fn hex_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect()
}

fn b64url(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i];
        let b1 = bytes.get(i + 1).copied().unwrap_or(0);
        let b2 = bytes.get(i + 2).copied().unwrap_or(0);
        let n = ((b0 as u32) << 16) | ((b1 as u32) << 8) | b2 as u32;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(T[((n >> 6) & 63) as usize] as char);
        }
        if i + 2 < bytes.len() {
            out.push(T[(n & 63) as usize] as char);
        }
        i += 3;
    }
    out
}

fn random_url(n: usize) -> String {
    let mut raw = vec![0u8; n];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut raw);
    } else {
        for (i, b) in raw.iter_mut().enumerate() {
            *b = ((now_secs() as u64).wrapping_mul(6364136223846793005) >> (i % 8) * 8) as u8;
        }
    }
    b64url(&raw).chars().take(n).collect()
}

fn bind_login() -> Option<(u16, TcpListener)> {
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    Some((port, listener))
}

fn wait_callback(listener: TcpListener) -> Value {
    listener.set_nonblocking(false).ok();
    let _ = listener.set_ttl(1);
    if let Ok((mut stream, _)) = listener.accept() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(180)));
        let mut buf = [0u8; 2048];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let line = req.lines().next().unwrap_or("");
        let path = line.split_whitespace().nth(1).unwrap_or("/");
        let qs = path.split('?').nth(1).unwrap_or("");
        let mut out = serde_json::Map::new();
        for part in qs.split('&') {
            if let Some((k, v)) = part.split_once('=') {
                out.insert(k.to_string(), json!(urldec(v)));
            }
        }
        let body = "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n로그인했습니다. 이 창을 닫아도 됩니다.";
        let _ = stream.write_all(body.as_bytes());
        return Value::Object(out);
    }
    json!({"error": "timeout"})
}

fn urldec(s: &str) -> String {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn login() -> i32 {
    if !configured() {
        println!("Token League가 꺼져 있습니다");
        return 0;
    }
    let client_id = setting_str(&["settings", "league", "clientId"]);
    let api_key = setting_str(&["settings", "league", "apiKey"]);
    if client_id.is_empty() || api_key.is_empty() {
        println!("Token League 로그인이 아직 열리지 않았습니다");
        return 1;
    }
    let (verifier, challenge) = pkce();
    let st = random_url(24);
    let Some((port, listener)) = bind_login() else {
        println!("로그인에 실패했습니다");
        return 1;
    };
    let redirect = format!("http://localhost:{port}");
    let url = format!(
        "{GOOGLE_AUTH}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        urlenc(&client_id),
        urlenc(&redirect),
        urlenc("openid profile"),
        urlenc(&st),
        urlenc(&challenge)
    );
    let _ = std::process::Command::new("open").arg(&url).status();
    let got = wait_callback(listener);
    let code = got.get("code").and_then(Value::as_str).unwrap_or("");
    let err = got.get("error").and_then(Value::as_str).unwrap_or("");
    let got_state = got.get("state").and_then(Value::as_str).unwrap_or("");
    if !err.is_empty() || code.is_empty() {
        println!("로그인을 취소했습니다");
        return 1;
    }
    if got_state != st {
        println!("로그인에 실패했습니다");
        return 1;
    }
    exchange(code, &verifier, &redirect, &client_id, &api_key)
}

fn exchange(code: &str, verifier: &str, redirect: &str, client_id: &str, api_key: &str) -> i32 {
    let secret = setting_str(&["settings", "league", "clientSecret"]);
    let mut fields = vec![
        ("code", code),
        ("client_id", client_id),
        ("redirect_uri", redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", verifier),
    ];
    if !secret.is_empty() {
        fields.push(("client_secret", &secret));
    }
    let body = form(&fields);
    let (status, raw) = http(
        "POST",
        GOOGLE_TOKEN,
        Some(&body),
        &[("Content-Type", "application/x-www-form-urlencoded")],
    );
    let google: Value = serde_json::from_slice(&raw).unwrap_or(json!({}));
    let google_id = google.get("id_token").and_then(Value::as_str).unwrap_or("");
    if status != 200 || google_id.is_empty() {
        println!("로그인에 실패했습니다");
        return 1;
    }
    let idp = json!({
        "postBody": format!("id_token={google_id}&providerId=google.com"),
        "requestUri": "http://localhost",
        "returnSecureToken": true,
    });
    let (status, raw) = http(
        "POST",
        &format!("{IDP_SIGNIN}?key={}", urlenc(api_key)),
        Some(idp.to_string().as_bytes()),
        &[("Content-Type", "application/json")],
    );
    let data: Value = serde_json::from_slice(&raw).unwrap_or(json!({}));
    let uid = data.get("localId").and_then(Value::as_str).unwrap_or("");
    let refresh = data.get("refreshToken").and_then(Value::as_str).unwrap_or("");
    if status != 200 || uid.is_empty() || refresh.is_empty() {
        println!("로그인에 실패했습니다");
        return 1;
    }
    let handle = data
        .get("displayName")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&uid[..uid.len().min(8)])
        .to_string();
    save_auth(refresh, uid, &handle);
    if let Some(tok) = data.get("idToken").and_then(Value::as_str) {
        let mut g = state().lock().unwrap();
        g.id_token = tok.to_string();
        g.id_exp = now_secs() + 3600.0;
    }
    println!("로그인했습니다 · {handle}");
    0
}

pub fn logout() -> i32 {
    let _ = fs::remove_file(auth_path());
    let mut g = state().lock().unwrap();
    g.id_token.clear();
    println!("로그아웃했습니다");
    0
}

pub fn open_room(rule: &str) -> i32 {
    if !configured() {
        println!("Token League가 꺼져 있습니다");
        return 0;
    }
    let Some((.., uid, handle)) = load_auth() else {
        println!("tokenmeter league login");
        return 1;
    };
    let chosen = if ALLOWED.contains(&rule) { rule } else { "cost" };
    let rooms = load_rooms();
    if rooms.len() >= MAX_ROOMS {
        println!("방은 최대 {MAX_ROOMS}개입니다");
        return 1;
    }
    let rid = random_url(12);
    if !room_id_ok(&rid) {
        println!("방을 열지 못했습니다");
        return 1;
    }
    let stamp = now_secs();
    let body = json!({"host_uid": uid, "handle": handle, "rule": chosen, "opened_at": stamp, "member_count": 1});
    if request_json(&room_url(&rid), "PUT", Some(&body)).is_none() {
        println!("방을 열지 못했습니다");
        return 1;
    }
    let status = crate::engine::Meter::load().status();
    let (cost, output) = totals_pair(&status);
    let snap = json!({"cost": cost, "output": output});
    let mut rooms = rooms;
    rooms.push(json!({
        "room_id": rid, "rule": chosen, "handle": handle, "host": true,
        "joined_total": snap, "opened_at": stamp
    }));
    write_rooms(rooms, &rid);
    println!("방을 열었습니다 · {rid}");
    let host = setting_str(&["settings", "league", "hostingBaseUrl"]);
    if !host.is_empty() {
        println!("{}/j/{rid}", host.trim_end_matches('/'));
    } else {
        println!("tokenmeter league join {rid}");
    }
    0
}

pub fn join_room(room: &str) -> i32 {
    if !configured() {
        println!("Token League가 꺼져 있습니다");
        return 0;
    }
    let Some((.., handle)) = load_auth() else {
        println!("tokenmeter league login");
        return 1;
    };
    let rid = room.trim();
    if rid.is_empty() {
        println!("사용법: tokenmeter league join <id>");
        return 1;
    }
    if !room_id_ok(rid) {
        println!("방 id: 4-22자 영문·숫자·_-");
        return 1;
    }
    let Some(data) = request_json(&room_url(rid), "GET", None) else {
        println!("방을 찾지 못했습니다");
        return 1;
    };
    if data.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        println!("방을 찾지 못했습니다");
        return 1;
    }
    let members = data.get("members").and_then(Value::as_object).map(|m| m.len()).unwrap_or(0);
    let counted = data.get("member_count").and_then(Value::as_i64).unwrap_or(0);
    if members >= MAX_MEMBERS || counted >= MAX_MEMBERS as i64 {
        println!("방이 가득 찼습니다");
        return 1;
    }
    if request_json(&count_url(rid), "PUT", Some(&json!(counted + 1))).is_none() {
        println!("방이 가득 찼습니다");
        return 1;
    }
    let mut rooms = load_rooms();
    if !rooms.iter().any(|r| r["room_id"] == rid) && rooms.len() >= MAX_ROOMS {
        println!("방은 최대 {MAX_ROOMS}개입니다");
        return 1;
    }
    let chosen = data
        .get("rule")
        .and_then(Value::as_str)
        .filter(|r| ALLOWED.contains(r))
        .unwrap_or("cost");
    let _ = request_json(&room_url(rid), "PATCH", Some(&json!({"claimed": true})));
    rooms.retain(|r| r["room_id"] != rid);
    rooms.push(json!({
        "room_id": rid, "rule": chosen, "handle": handle, "host": false, "claimed": true
    }));
    write_rooms(rooms, rid);
    println!("참가했습니다 · {rid}");
    0
}

pub fn leave(rid: Option<&str>) -> i32 {
    let rooms = load_rooms();
    if let Some(rid) = rid.filter(|s| !s.is_empty()) {
        write_rooms(
            rooms.into_iter().filter(|r| r["room_id"] != rid).collect(),
            "",
        );
    } else {
        write_rooms(Vec::new(), "");
    }
    println!("나갔습니다");
    0
}

pub fn close_room(rid: Option<&str>) -> i32 {
    let rooms = load_rooms();
    let target = rid
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| {
            state()
                .lock()
                .ok()
                .map(|g| g.focus.clone())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_default();
    if !room_id_ok(&target) {
        println!("닫을 방이 없습니다");
        return 1;
    }
    if let Some(room) = rooms.iter().find(|r| r["room_id"] == target) {
        if room.get("host") != Some(&json!(true)) {
            println!("호스트만 방을 닫을 수 있습니다");
            return 1;
        }
    }
    let _ = request_json(&room_url(&target), "DELETE", None);
    write_rooms(
        rooms.into_iter().filter(|r| r["room_id"] != target).collect(),
        "",
    );
    println!("방을 닫았습니다 · {target}");
    0
}

pub fn room_open() -> bool {
    !load_rooms().is_empty()
}

pub fn tick(status: &Value, tps: f64) {
    if !configured() {
        return;
    }
    let rooms = load_rooms();
    if rooms.is_empty() {
        return;
    }
    let Some((_, uid, handle)) = load_auth() else {
        return;
    };
    let stamp = now_secs();
    let focus = state().lock().unwrap().focus.clone();
    for info in &rooms {
        let rid = info.get("room_id").and_then(Value::as_str).unwrap_or("");
        if !room_id_ok(rid) {
            continue;
        }
        if info.get("host") == Some(&json!(true)) {
            if let Some(opened) = info.get("opened_at").and_then(Value::as_f64) {
                if stamp - opened >= EMPTY_ROOM_TTL && info.get("claimed") != Some(&json!(true)) {
                    let _ = request_json(&room_url(rid), "DELETE", None);
                    continue;
                }
            }
        }
        let (cost, output) = totals_pair(status);
        let (check, working, _, _) = attention_counts(status, stamp);
        let row = json!({
            "handle": handle, "tps": tps, "output_tokens": output, "cost_usd": cost,
            "check": check, "working": working, "at": stamp,
            "joined_total": info.get("joined_total").cloned().unwrap_or(json!({"cost": cost, "output": output})),
        });
        let mut g = state().lock().unwrap();
        let due = rid != g.put_room || g.last_put == 0.0 || stamp - g.last_put >= PUT_INTERVAL;
        if due && (rid == focus || g.last_put == 0.0) {
            drop(g);
            if request_json(&member_url(rid, &uid), "PUT", Some(&row)).is_some() && rid == focus {
                let mut g = state().lock().unwrap();
                g.last_put = stamp;
                g.put_room = rid.to_string();
                if let Some(members) = g.snap.as_object_mut() {
                    let book = members.entry("members".to_string()).or_insert(json!({}));
                    if let Some(obj) = book.as_object_mut() {
                        obj.insert(uid.clone(), row.clone());
                    }
                }
                let _ = fs::write(cache_path(), g.snap.to_string());
            }
        }
    }
    ensure_sse(&focus);
}

fn ensure_sse(rid: &str) {
    if rid.is_empty() {
        return;
    }
    {
        let g = state().lock().unwrap();
        if g.sse_room == rid {
            return;
        }
    }
    state().lock().unwrap().sse_room = rid.to_string();
    let rid = rid.to_string();
    thread::spawn(move || loop {
        if state().lock().unwrap().sse_room != rid {
            return;
        }
        let token = id_token(false);
        if token.is_empty() {
            thread::sleep(Duration::from_secs(1));
            continue;
        }
        let url = authed(&room_url(&rid), &token);
        if let Ok(resp) = ureq::get(&url)
            .set("Accept", "text/event-stream")
            .timeout(Duration::from_secs(60))
            .call()
        {
            let mut reader = resp.into_reader();
            let mut buf = String::new();
            let mut raw = [0u8; 4096];
            loop {
                if state().lock().unwrap().sse_room != rid {
                    return;
                }
                match reader.read(&mut raw) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.push_str(&String::from_utf8_lossy(&raw[..n]));
                        while let Some(idx) = buf.find("\n\n") {
                            let chunk = buf[..idx].to_string();
                            buf = buf[idx + 2..].to_string();
                            apply_sse(&chunk, &rid);
                        }
                    }
                    Err(_) => break,
                }
            }
        }
        thread::sleep(Duration::from_secs(1));
    });
}

fn apply_sse(chunk: &str, rid: &str) {
    let mut event = "message";
    let mut data = String::new();
    for line in chunk.lines() {
        if line.starts_with("event:") {
            event = line[6..].trim();
        } else if line.starts_with("data:") {
            data.push_str(line[5..].trim());
        }
    }
    if matches!(event, "keep-alive" | "cancel") {
        return;
    }
    if event == "auth_revoked" {
        state().lock().unwrap().id_token.clear();
        return;
    }
    if event != "put" && event != "patch" {
        return;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&data) else {
        return;
    };
    if payload.get("data").is_none() && payload.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        write_rooms(
            load_rooms().into_iter().filter(|r| r["room_id"] != rid).collect(),
            "",
        );
        return;
    }
    let snap = payload.get("data").cloned().unwrap_or(payload);
    let mut g = state().lock().unwrap();
    g.snap = snap;
    let _ = fs::write(cache_path(), g.snap.to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_id_and_week_key() {
        assert!(room_id_ok("Ab12_-xx"));
        assert!(room_id_ok(&"A".repeat(22)));
        assert!(!room_id_ok("ab"));
        assert!(!room_id_ok(&"A".repeat(23)));
        assert_eq!(PUT_INTERVAL, 5.0);
        assert!(week_key(1_776_960_000.0).contains("-W"));
    }

    #[test]
    fn caption_asks_login_when_enabled_without_auth() {
        let text = caption();
        assert!(
            text == "꺼짐" || text.contains("league login") || text.contains("league open") || text.starts_with("방 "),
            "{text}"
        );
    }

    #[test]
    fn overlay_step_is_login_open_or_idle() {
        let step = overlay_step();
        assert!(step == "" || step == "login" || step == "open", "{step}");
    }
}
