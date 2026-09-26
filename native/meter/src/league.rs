//! Token League 방. GitHub로 로그인하고(`github`), 방은 서버가, 실시간 tok/s는 멤버끼리 iroh로(`live`) 주고받는다.
//! 파일(data_dir, 모두 0600): league-auth.json {uid, handle, since}, league-key(iroh 비밀키),
//! league.json {rooms, focus, relay, invite, live}(방 목록), league-cache.json {members}(오버레이가 읽는 행).
//! CLI 명령은 league.json을 고치고, 데몬은 그 파일을 읽어 연결 대상을 맞춘다.

use crate::github;
use crate::live::{Live, Peer, Row};
use crate::server::{self, ApiError};
use iroh::{EndpointAddr, RelayUrl, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokenmeter_hook::data_dir;
use tokenmeter_protocol::{room_id_ok, AuthRequest, AuthResponse, Member, Room, RoomList};

const DEFAULT_INVITE: &str = "https://tokenmeter.online/j/";
const REFRESH_S: f64 = 900.0;
/// 모르는 상대 때문에 방 목록을 새로 받는 최소 간격. 서버의 방 요청 한도(사용자당 0.1/s)를 기기 여럿이 나눠 쓴다.
const UNKNOWN_REFRESH_S: f64 = 60.0;
/// 오버레이는 15초 넘은 행을 숨긴다. 여러 기기의 합도 같은 기준으로 센다.
const FRESH_S: f64 = 15.0;
const PRIVACY: &str = "Members see your GitHub login and live output rate. While you are in a room, anyone who knows your endpoint id (current or past members) gets your public IP and local addresses when they connect; leaving a room or logging out changes the key.";

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Auth {
    pub uid: String,
    pub handle: String,
    /// 로그인한 유닉스 초. 공유가 꺼져 있으면 이때부터 합계 셀을 보낸다.
    pub since: f64,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct LocalRoom {
    pub room_id: String,
    pub host: bool,
    pub members: Vec<Member>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Book {
    pub rooms: Vec<LocalRoom>,
    pub focus: String,
    pub relay: String,
    pub invite: String,
    pub live: bool,
}

fn path(name: &str) -> PathBuf {
    data_dir().join(name)
}

/// 0600 임시 파일에 쓰고 바꾼다. 디렉터리는 만들지 않는다(`uninstall --purge` 뒤 데몬이 되살리지 않게).
fn write_private(name: &str, text: &str) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path(&format!("{name}.tmp"));
    fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?.write_all(text.as_bytes())?;
    fs::rename(tmp, path(name))
}

pub fn auth() -> Option<Auth> {
    let a: Auth = serde_json::from_str(&fs::read_to_string(path("league-auth.json")).ok()?).ok()?;
    Some(a).filter(|a| !a.uid.is_empty())
}

/// 서버 주소가 있으면 켜짐. 오버레이가 프레임마다 부르므로 설정 파일은 5초에 한 번만 읽는다.
pub fn enabled() -> bool {
    if cfg!(test) {
        return !server::base().is_empty();
    }
    static SEEN: Mutex<(f64, bool)> = Mutex::new((0.0, false));
    let now = crate::watch::now_secs();
    let mut seen = SEEN.lock().unwrap();
    if now >= seen.0 {
        *seen = (now + 5.0, !server::base().is_empty());
    }
    seen.1
}

pub fn has_auth() -> bool {
    auth().is_some()
}

/// 로그인한 시각. 없으면 지금(지금 칸만 보낸다).
pub fn since() -> f64 {
    auth().map(|a| a.since).filter(|s| *s > 0.0).unwrap_or_else(crate::watch::now_secs)
}

fn book() -> Book {
    fs::read_to_string(path("league.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save_book(b: &Book) {
    let _ = write_private("league.json", &serde_json::to_string(b).unwrap_or_default());
}

fn focus_of(b: &Book) -> String {
    if b.rooms.iter().any(|r| r.room_id == b.focus) {
        b.focus.clone()
    } else {
        b.rooms.first().map(|r| r.room_id.clone()).unwrap_or_default()
    }
}

/// 오버레이가 `room_id`·`host`를 읽는다.
pub fn list_rooms() -> Vec<Value> {
    if !has_auth() {
        return Vec::new();
    }
    book().rooms.iter().filter_map(|r| serde_json::to_value(r).ok()).collect()
}

pub fn current_focus() -> String {
    focus_of(&book())
}

pub fn focus_room(rid: &str) {
    let mut b = book();
    if b.rooms.iter().any(|r| r.room_id == rid) {
        b.focus = rid.into();
        save_book(&b);
    }
}

pub fn invite_link(rid: &str) -> String {
    if rid.is_empty() {
        return String::new();
    }
    let base = book().invite;
    format!("{}{rid}", if base.is_empty() { DEFAULT_INVITE } else { &base })
}

/// 로그인한 상태로 방에 들어가 있으면 데몬이 유휴 종료하지 않는다. 데몬 루프(200ms)가 부르므로
/// 파일을 읽지 않고 tick이 5초마다 본 값을 쓴다.
pub fn room_open() -> bool {
    ticker().lock().unwrap().in_room
}

pub fn overlay_step() -> &'static str {
    if !enabled() {
        ""
    } else if !has_auth() {
        "login"
    } else if list_rooms().is_empty() {
        "open"
    } else {
        ""
    }
}

pub fn caption() -> String {
    if !enabled() {
        return "off — settings.league.server is empty".into();
    }
    let Some(a) = auth() else {
        return "not logged in — tokenmeter league login".into();
    };
    let n = book().rooms.len();
    format!("@{} · {n} room{}", a.handle, if n == 1 { "" } else { "s" })
}

/// iroh 비밀키. 없으면 만든다(처음 로그인, 또는 방을 나가거나 로그아웃해 지운 뒤). EndpointId는 로그인과 동기화 요청에 같이 간다.
pub fn key() -> std::io::Result<SecretKey> {
    if let Some(key) = fs::read_to_string(path("league-key")).ok().and_then(|t| t.trim().parse().ok()) {
        return Ok(key);
    }
    let key = SecretKey::generate();
    fs::create_dir_all(data_dir())?;
    write_private("league-key", &key.to_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>())?;
    Ok(key)
}

/// 로그인했을 때만 이 기기의 EndpointId. 익명 공유 업로드에는 넣지 않는다.
pub fn endpoint_id() -> Option<String> {
    if has_auth() {
        key().ok().map(|k| k.public().to_string())
    } else {
        None
    }
}

/// 로그아웃·계정 삭제 뒤 로컬 리그 파일을 지운다. 키도 지워 다음 로그인은 새 EndpointId를 쓴다.
/// iroh는 연결을 등록하자마자 내 주소 후보를 보내므로, 예전 방 멤버가 옛 EndpointId로 내 주소를 받아 가지 못하게 한다.
pub fn forget() {
    for name in ["league-auth.json", "league.json", "league-cache.json", "league-key"] {
        let _ = fs::remove_file(path(name));
    }
}

fn reason(e: &ApiError) -> String {
    match e {
        ApiError::Off => "settings.league.server is empty".into(),
        ApiError::Offline => "the league server did not answer".into(),
        ApiError::Upgrade => "this version is too old — tokenmeter update now".into(),
        ApiError::Status(_, code) if code == "room_full" => "the room is full (20 people)".into(),
        ApiError::Status(_, code) if code == "too_many_rooms" => "you are already in 8 rooms".into(),
        ApiError::Status(401, _) => "log in first: tokenmeter league login".into(),
        ApiError::Status(403, _) => "not allowed (only the host can close a room)".into(),
        ApiError::Status(404, _) => "no such room".into(),
        ApiError::Status(code, err) => format!("server error {code} {err}"),
    }
}

fn local(r: Room, uid: &str) -> LocalRoom {
    LocalRoom { host: r.host_id.to_string() == uid, room_id: r.id, members: r.members }
}

/// 서버에서 설정과 방 목록을 받아 league.json을 새로 쓴다. 포커스는 남아 있으면 그대로다.
/// 서버가 401이면(다른 곳에서 로그아웃·계정 삭제) 로컬 로그인도 지운다.
pub fn refresh() -> Result<(), ApiError> {
    let Some(me) = auth() else { return Ok(()) };
    let token = server::device_token().ok_or(ApiError::Status(401, "unauthorized".into()))?;
    let cfg = server::config()?;
    let list: RoomList = match server::json("GET", "/v1/rooms", &token, None) {
        Err(ApiError::Status(401, e)) => {
            forget();
            return Err(ApiError::Status(401, e));
        }
        other => other?,
    };
    // ponytail: CLI가 방을 여는 사이에 데몬이 옛 목록을 쓰면 다음 새로 고침(15분)까지 그 방이 빠진다. 드물다.
    let b = Book {
        rooms: list.rooms.into_iter().map(|r| local(r, &me.uid)).collect(),
        focus: book().focus,
        relay: cfg.relay,
        invite: cfg.invite,
        live: cfg.live,
    };
    save_book(&b);
    Ok(())
}

/// 로그인 확인과 기기 토큰. 안 됐으면 안내를 찍는다.
fn ready() -> Option<String> {
    if !has_auth() {
        println!("  Log in first: tokenmeter league login");
        return None;
    }
    let token = server::device_token();
    if token.is_none() {
        println!("  Log in again: tokenmeter league login");
    }
    token
}

/// 방을 league.json에 넣고 포커스를 옮긴다. 데몬이 이 파일을 보고 연결 대상을 바꾼다.
fn remember(room: Room) {
    let Some(me) = auth() else { return };
    let mut b = book();
    b.rooms.retain(|r| r.room_id != room.id);
    b.focus = room.id.clone();
    b.rooms.push(local(room, &me.uid));
    save_book(&b);
}

fn notify_too() -> bool {
    !cfg!(test) && !std::io::stdout().is_terminal()
}

fn copy(text: &str) {
    if cfg!(test) {
        return;
    }
    for (cmd, args) in [("pbcopy", &[][..]), ("wl-copy", &[][..]), ("xclip", &["-selection", "clipboard"][..])] {
        let Ok(mut child) = std::process::Command::new(cmd).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
        else {
            continue;
        };
        if let Some(mut input) = child.stdin.take() {
            let _ = input.write_all(text.as_bytes());
        }
        if child.wait().is_ok_and(|s| s.success()) {
            return;
        }
    }
}

fn open_url(url: &str) {
    if cfg!(test) {
        return;
    }
    let cmd = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let _ = std::process::Command::new(cmd).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

pub fn login() -> i32 {
    login_with(github::GITHUB)
}

/// 스펙 §2 순서. 오버레이 버튼으로 시작해 터미널이 없으면 같은 안내를 데스크톱 알림으로도 띄운다.
fn login_with(gh: &str) -> i32 {
    let notify = notify_too();
    let say = |msg: &str| {
        println!("  {msg}");
        if notify {
            crate::daemon::notify("Token League", msg);
        }
    };
    if !enabled() {
        say("The league is off: settings.league.server is empty.");
        return 1;
    }
    let token = match server::ensure_device() {
        Ok(t) => t,
        Err(e) => return fail(&say, &format!("Can't reach the league server: {}", reason(&e))),
    };
    let key = match key() {
        Ok(k) => k,
        Err(e) => return fail(&say, &format!("Can't write the league key: {e}")),
    };
    let code = match github::start(gh) {
        Ok(c) => c,
        Err(e) => return fail(&say, &format!("GitHub login did not start: {e}")),
    };
    copy(&code.user_code);
    open_url(&code.verification_uri);
    say(&format!("Enter code {} at {} (copied to the clipboard).", code.user_code, code.verification_uri));
    let github_token = match github::wait_token(gh, &code, |s| std::thread::sleep(Duration::from_secs(s))) {
        Ok(t) => t,
        Err(e) => return fail(&say, &format!("GitHub login stopped: {e}")),
    };
    // GitHub 토큰은 이 요청 한 번에만 쓰고 버린다. 서버가 확인한 뒤 폐기한다.
    let req = AuthRequest { github_token, endpoint_id: key.public().to_string() };
    let me: AuthResponse = match server::json("POST", "/v1/auth/github", &token, serde_json::to_value(&req).ok()) {
        Ok(me) => me,
        Err(e) => return fail(&say, &format!("The league server refused the login: {}", reason(&e))),
    };
    drop(req);
    let a = Auth { uid: me.user_id.to_string(), handle: me.login.clone(), since: crate::watch::now_secs().floor() };
    if let Err(e) = write_private("league-auth.json", &serde_json::to_string(&a).unwrap_or_default()) {
        return fail(&say, &format!("Can't save the login: {e}"));
    }
    let _ = refresh();
    say(&format!("Logged in as @{}. Next: tokenmeter league open, or join a friend's invite link.", me.login));
    0
}

fn fail(say: &impl Fn(&str), msg: &str) -> i32 {
    say(msg);
    1
}

pub fn logout() -> i32 {
    let told = server::device_token().map(|t| server::json::<()>("POST", "/v1/auth/logout", &t, None));
    forget();
    match told {
        Some(Err(e)) if e != ApiError::Status(401, "unauthorized".into()) => {
            println!("  Logged out here, but the server did not hear it ({}). Run it again later.", reason(&e))
        }
        _ => println!("  Logged out. Your rooms stay; log in again to see them."),
    }
    0
}

/// 방은 규칙이 없다(규칙은 경기가 가진다). 인자는 오버레이 호출부를 그대로 두려고 남긴다.
pub fn open_room(_rule: &str) -> i32 {
    let Some(token) = ready() else { return 1 };
    match server::json::<Room>("POST", "/v1/rooms", &token, None) {
        Ok(room) => {
            let id = room.id.clone();
            remember(room);
            println!("  Room {id} is open. Invite: {}", invite_link(&id));
            println!("  {PRIVACY}");
            0
        }
        Err(e) => {
            println!("  Couldn't open a room: {}", reason(&e));
            1
        }
    }
}

/// `<id>`나 초대 링크(`https://tokenmeter.online/j/<id>`) 둘 다 받는다.
pub fn join_room(room: &str) -> i32 {
    let id = room.trim().trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if !room_id_ok(id) {
        println!("  usage: tokenmeter league join <room id or invite link>");
        return 1;
    }
    let Some(token) = ready() else { return 1 };
    match server::json::<Room>("POST", &format!("/v1/rooms/{id}/join"), &token, None) {
        Ok(room) => {
            let n = room.members.len();
            remember(room);
            println!("  Joined {id} ({n} in the room).");
            println!("  {PRIVACY}");
            0
        }
        Err(e) => {
            println!("  Couldn't join: {}", reason(&e));
            1
        }
    }
}

pub fn leave(rid: Option<&str>) -> i32 {
    room_call(rid, "POST", "/leave", "Left")
}

pub fn close_room(rid: Option<&str>) -> i32 {
    room_call(rid, "DELETE", "", "Closed")
}

fn room_call(rid: Option<&str>, method: &str, suffix: &str, done: &str) -> i32 {
    let id = rid.map(str::to_string).unwrap_or_else(current_focus);
    if id.is_empty() {
        println!("  You are not in a room.");
        return 1;
    }
    let Some(token) = ready() else { return 1 };
    match server::json::<()>(method, &format!("/v1/rooms/{id}{suffix}"), &token, None) {
        Ok(()) | Err(ApiError::Status(404, _)) => {
            let mut b = book();
            b.rooms.retain(|r| r.room_id != id);
            save_book(&b);
            // 이 방 멤버였던 사람이 옛 EndpointId로 내 주소를 받아 가지 못하게 키를 바꾼다. 데몬이 새 키로 다시 띄운다.
            let _ = fs::remove_file(path("league-key"));
            println!("  {done} {id}.");
            0
        }
        Err(e) => {
            println!("  Couldn't do that: {}", reason(&e));
            1
        }
    }
}

/// `tokenmeter league`: 로그인, 방, 초대 링크. 네트워크를 쓰지 않는다.
pub fn show() -> i32 {
    println!("Token League");
    println!("  login  : {}", caption());
    let b = if has_auth() { book() } else { Book::default() };
    let focus = focus_of(&b);
    for r in &b.rooms {
        let names: Vec<&str> = r.members.iter().map(|m| m.login.as_str()).collect();
        let mark = if r.room_id == focus { "▸" } else { " " };
        println!("  {mark} {}{} · {}", r.room_id, if r.host { " (host)" } else { "" }, names.join(", "));
    }
    if !focus.is_empty() {
        println!("  invite : {}", invite_link(&focus));
    }
    println!("  usage  : tokenmeter league login | logout | open | join <id|link> | leave [id] | close [id]");
    0
}

/// 오버레이의 로그인 버튼. 터미널이 없으니 login이 코드를 클립보드와 알림으로도 알린다.
pub fn spawn_login_cli() {
    let _ = std::process::Command::new(crate::install::meter_bin())
        .args(["league", "login"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
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

// ---- 데몬 쪽: 실시간 연결과 오버레이 행 ----

#[derive(Default)]
struct Ticker {
    next: f64,
    next_look: f64,
    refresh_at: f64,
    unknown_at: f64,
    me: Option<Auth>,
    in_room: bool,
    names: HashMap<String, String>,
    relay: Option<RelayUrl>,
    live: Option<Live>,
}

fn ticker() -> &'static Mutex<Ticker> {
    static T: OnceLock<Mutex<Ticker>> = OnceLock::new();
    T.get_or_init(Mutex::default)
}

/// 실시간 릴레이. `settings.league.relay`가 있으면 그것(시험·자체 서버용, 서버의 live 스위치도 무시),
/// 없으면 서버가 live를 켰을 때만 서버 설정의 relay.
fn relay_for(b: &Book) -> Option<RelayUrl> {
    let own = crate::watch::setting_str(&["settings", "league", "relay"]);
    let url = if !own.is_empty() { own } else if b.live { b.relay.clone() } else { String::new() };
    url.parse().ok()
}

/// 포커스된 방의 다른 멤버 기기들. 내 다른 기기는 뺀다(내 행은 로컬 tok/s로 쓴다).
/// 모두 같은 릴레이를 쓰므로 주소는 릴레이만 주면 iroh가 직접 경로로 옮겨 간다.
fn peers(b: &Book, me: &str, relay: &RelayUrl) -> Vec<Peer> {
    let focus = focus_of(b);
    let mut out = Vec::new();
    for m in b.rooms.iter().filter(|r| r.room_id == focus).flat_map(|r| &r.members) {
        let uid = m.user_id.to_string();
        if uid == me {
            continue;
        }
        for id in m.endpoints.iter().filter_map(|e| e.parse().ok()) {
            out.push(Peer { uid: uid.clone(), addr: EndpointAddr::new(id).with_relay_url(relay.clone()) });
        }
    }
    out
}

/// 오버레이가 읽는 `members[uid] = {handle, tps, at}`. 기기가 여럿인 멤버는 15초 안의 값을 더한다.
pub fn cache(me: &Auth, my_tps: f64, rows: &[Row], names: &HashMap<String, String>, now: f64) -> Value {
    let mut members = serde_json::Map::new();
    members.insert(me.uid.clone(), json!({"handle": me.handle, "tps": my_tps, "at": now}));
    for r in rows.iter().filter(|r| now - r.at <= FRESH_S && r.uid != me.uid) {
        let handle = names.get(&r.uid).cloned().unwrap_or_else(|| r.uid.clone());
        let row = members.entry(r.uid.clone()).or_insert_with(|| json!({"handle": handle, "tps": 0.0, "at": 0.0}));
        row["tps"] = json!(row["tps"].as_f64().unwrap_or(0.0) + r.tps);
        row["at"] = json!(row["at"].as_f64().unwrap_or(0.0).max(r.at));
    }
    json!({ "members": members })
}

/// 데몬 루프(200ms)에서 부른다. 1초마다 내 tok/s와 league-cache.json, 5초마다 league.json·로그인을 다시 읽어
/// 연결 대상을 맞춘다. 방 목록은 15분마다, 모르는 상대가 붙으면 60초에 한 번까지 따로 스레드에서 받는다.
pub fn tick(_status: &Value, tps: f64) {
    let now = crate::watch::now_secs();
    let mut t = ticker().lock().unwrap();
    if now < t.next {
        return;
    }
    t.next = now + 1.0;
    if now >= t.next_look {
        t.next_look = now + 5.0;
        t.me = auth().filter(|_| enabled());
        let Some(me) = t.me.clone() else {
            (t.live, t.relay, t.in_room) = (None, None, false);
            return;
        };
        let stranger = now - t.unknown_at >= UNKNOWN_REFRESH_S && t.live.as_ref().is_some_and(Live::take_unknown);
        if now >= t.refresh_at || stranger {
            (t.refresh_at, t.unknown_at) = (now + REFRESH_S, now);
            std::thread::spawn(|| {
                if let Err(e) = refresh() {
                    eprintln!("[league] rooms: {e:?}");
                }
                // 받은 목록을 다음 tick에 곧바로 읽는다. 모르는 상대(새 멤버)가 끊기기 전에 확인되게.
                ticker().lock().unwrap().next_look = 0.0;
            });
        }
        let b = book();
        t.in_room = !b.rooms.is_empty();
        t.names = b.rooms.iter().flat_map(|r| &r.members).map(|m| (m.user_id.to_string(), m.login.clone())).collect();
        let relay = relay_for(&b).filter(|_| t.in_room);
        if relay != t.relay {
            (t.live, t.relay) = (None, relay);
        }
        // 방을 나가거나 로그아웃하면 키가 바뀐다(room_call, forget). 새 키로 다시 띄운다.
        if t.live.as_ref().is_some_and(|l| key().map_or(true, |k| k.public() != l.id())) {
            t.live = None;
        }
        if t.live.is_none() {
            if let Some(url) = t.relay.clone() {
                t.live = key().map_err(|e| e.to_string()).and_then(|k| Live::start(k, Some(url))).map_err(|e| eprintln!("[league] live: {e}")).ok();
            }
        }
        if let (Some(live), Some(url)) = (&t.live, &t.relay) {
            live.set_peers(peers(&b, &me.uid, url));
        }
    }
    let (Some(me), true) = (t.me.clone(), t.in_room) else { return };
    let rows = t.live.as_ref().map(|l| {
        l.set_tps(tps);
        l.rows()
    });
    let _ = write_private("league-cache.json", &cache(&me, tps, &rows.unwrap_or_default(), &t.names, now).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::fake::{serve, use_server};

    fn logged_in(uid: &str, handle: &str) {
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(path("league-auth.json"), json!({"uid": uid, "handle": handle, "since": 1}).to_string()).unwrap();
        fs::write(path("device.json"), r#"{"device_id":"d1","token":"tmd_x"}"#).unwrap();
    }

    const ROOM: &str = r#"{"id":"aZ0_-aZ0_-aZ","host_id":42,"members":[{"user_id":42,"login":"alice","endpoints":[]},{"user_id":7,"login":"bob","endpoints":[]}]}"#;
    const ROOMS: &str = r#"{"rooms":[{"id":"aZ0_-aZ0_-aZ","host_id":42,"members":[{"user_id":42,"login":"alice","endpoints":[]}]}]}"#;
    const CONFIG: &str = r#"{"relay":"https://relay.example","invite":"https://tokenmeter.online/j/","active_s":60,"idle_s":900,"live":true}"#;

    #[test]
    fn login_saves_who_i_am_and_never_the_github_token() {
        let (_g, _tmp) = crate::test_home("league-login");
        let (gh, _) = serve(vec![
            (200, r#"{"device_code":"dc1","user_code":"ABCD-1234","verification_uri":"https://github.com/login/device","expires_in":900,"interval":0}"#),
            (200, r#"{"access_token":"gho_secret","token_type":"bearer","scope":""}"#),
        ]);
        let (url, seen) = serve(vec![(200, r#"{"user_id":42,"login":"alice"}"#), (200, CONFIG), (200, r#"{"rooms":[]}"#)]);
        use_server(&url);
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(path("device.json"), r#"{"device_id":"d1","token":"tmd_x"}"#).unwrap();
        assert_eq!(login_with(&gh), 0);
        let sent = seen.recv().unwrap();
        let endpoint = key().unwrap().public().to_string();
        assert!(sent.starts_with("POST /v1/auth/github") && sent.contains("gho_secret") && sent.contains(&endpoint), "{sent}");
        assert_eq!(auth().map(|a| (a.uid, a.handle)), Some(("42".into(), "alice".into())));
        use std::os::unix::fs::PermissionsExt;
        for name in ["league-auth.json", "league-key"] {
            assert_eq!(fs::metadata(path(name)).unwrap().permissions().mode() & 0o777, 0o600, "{name}");
        }
        for entry in fs::read_dir(data_dir()).unwrap() {
            let text = fs::read_to_string(entry.unwrap().path()).unwrap_or_default();
            assert!(!text.contains("gho_secret"), "GitHub 토큰은 디스크에 쓰지 않는다");
        }
        assert_eq!((overlay_step(), caption().as_str()), ("open", "@alice · 0 rooms"));
    }

    #[test]
    fn room_commands_keep_league_json_in_the_overlay_shape() {
        let (_g, _tmp) = crate::test_home("league-rooms");
        let (url, seen) = serve(vec![(201, ROOM), (204, "")]);
        use_server(&url);
        assert_eq!(join_room("https://tokenmeter.online/j/aZ0_-aZ0_-aZ"), 1, "로그인 전");
        logged_in("7", "bob");
        assert_eq!(join_room("https://tokenmeter.online/j/aZ0_-aZ0_-aZ/"), 0);
        assert!(seen.recv().unwrap().starts_with("POST /v1/rooms/aZ0_-aZ0_-aZ/join"));
        let rooms = list_rooms();
        assert_eq!((rooms[0]["room_id"].as_str(), rooms[0]["host"].as_bool()), (Some("aZ0_-aZ0_-aZ"), Some(false)));
        assert_eq!(current_focus(), "aZ0_-aZ0_-aZ");
        assert_eq!(invite_link(&current_focus()), "https://tokenmeter.online/j/aZ0_-aZ0_-aZ");
        assert!(overlay_step().is_empty());
        let old_key = key().unwrap().public();
        assert_eq!(leave(None), 0);
        assert!(seen.recv().unwrap().starts_with("POST /v1/rooms/aZ0_-aZ0_-aZ/leave"));
        assert!(list_rooms().is_empty() && !room_open());
        assert_ne!(key().unwrap().public(), old_key, "방을 나가면 키를 바꾼다");
        assert_eq!(join_room("../etc"), 1, "형식이 틀린 id는 보내지 않는다");
    }

    #[test]
    fn refresh_follows_the_server_and_a_401_logs_out() {
        let (_g, _tmp) = crate::test_home("league-refresh");
        logged_in("42", "alice");
        key().unwrap();
        let (url, _) = serve(vec![(200, CONFIG), (200, ROOMS), (200, CONFIG), (401, r#"{"error":"unauthorized","message":""}"#)]);
        use_server(&url);
        refresh().unwrap();
        let b = book();
        assert_eq!((b.rooms.len(), b.rooms[0].host, b.relay.as_str(), b.live), (1, true, "https://relay.example", true));
        assert!(refresh().is_err());
        assert!(!has_auth() && list_rooms().is_empty(), "서버가 모르는 로그인은 지운다");
        assert!(!path("league-key").exists(), "키도 지운다(다음 로그인은 새 EndpointId)");
    }

    #[test]
    fn peers_are_the_focused_room_minus_my_devices() {
        let relay: RelayUrl = "https://relay.example".parse().unwrap();
        let (a, b) = (SecretKey::generate().public().to_string(), SecretKey::generate().public().to_string());
        let room = |id: &str, members: Vec<Member>| LocalRoom { room_id: id.into(), host: false, members };
        let m = |uid: i64, e: &[&str]| Member { user_id: uid, login: format!("u{uid}"), endpoints: e.iter().map(|s| s.to_string()).collect() };
        let book = Book {
            rooms: vec![room("r1", vec![m(1, &[&a]), m(2, &[&b, "bad"])]), room("r2", vec![m(3, &[&b])])],
            focus: "r1".into(),
            ..Book::default()
        };
        let got = peers(&book, "1", &relay);
        assert_eq!(got.len(), 1, "내 기기와 못 읽는 id는 빠진다");
        assert_eq!((got[0].uid.as_str(), got[0].addr.id.to_string()), ("2", b.clone()));
    }

    #[test]
    fn cache_sums_fresh_rows_per_member_and_keeps_mine() {
        let me = Auth { uid: "1".into(), handle: "me".into(), since: 0.0 };
        let names = HashMap::from([("2".to_string(), "bob".to_string())]);
        let row = |uid: &str, tps: f64, at: f64| Row { uid: uid.into(), tps, at };
        let v = cache(&me, 5.0, &[row("2", 3.0, 100.0), row("2", 4.0, 99.0), row("3", 9.0, 80.0)], &names, 100.0);
        assert_eq!(v["members"]["1"], json!({"handle": "me", "tps": 5.0, "at": 100.0}));
        assert_eq!(v["members"]["2"], json!({"handle": "bob", "tps": 7.0, "at": 100.0}), "두 기기의 합");
        assert!(v["members"].get("3").is_none(), "15초 넘은 행은 뺀다");
    }

    #[test]
    fn two_meters_fill_league_cache_json() {
        let (_g, _tmp) = crate::test_home("league-cache");
        let a = Live::start(SecretKey::generate(), None).unwrap();
        let b = Live::start(SecretKey::generate(), None).unwrap();
        a.set_peers(vec![Peer { uid: "2".into(), addr: b.addr() }]);
        b.set_peers(vec![Peer { uid: "1".into(), addr: a.addr() }]);
        a.set_tps(21.0);
        let end = std::time::Instant::now() + Duration::from_secs(20);
        while b.rows().is_empty() {
            assert!(std::time::Instant::now() < end, "b가 a의 값을 받지 못함");
            std::thread::sleep(Duration::from_millis(50));
        }
        fs::create_dir_all(data_dir()).unwrap();
        let me = Auth { uid: "2".into(), handle: "bob".into(), since: 0.0 };
        let names = HashMap::from([("1".to_string(), "alice".to_string())]);
        write_private("league-cache.json", &cache(&me, 0.0, &b.rows(), &names, crate::watch::now_secs()).to_string()).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(path("league-cache.json")).unwrap()).unwrap();
        assert_eq!((v["members"]["1"]["handle"].as_str(), v["members"]["1"]["tps"].as_f64()), (Some("alice"), Some(21.0)));
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
