//! TokenMeter 훅 도메인 — 에이전트가 매 이벤트마다 띄우는 빠른 경로.
//! 표준 라이브러리 + serde_json 만 쓴다. GUI/네트워크/설정 YAML 금지.

use serde_json::{json, Map, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub const STOP_EVENTS: &[&str] = &["SessionEnd", "session.deleted", "sessionEnd"];
const CHECK_EVENTS: &[&str] = &[
    "PermissionRequest",
    "permission.asked",
    "permission.v2.asked",
    "question.asked",
    "question.v2.asked",
];
const WAIT_EVENTS: &[&str] = &["Stop", "session.idle", "stop"];
const WORK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "session.created",
    "permission.replied",
    "permission.v2.replied",
    "question.replied",
    "question.v2.replied",
    "sessionStart",
    "beforeSubmitPrompt",
    "afterAgentResponse",
    "afterAgentThought",
    "postToolUse",
];
const ATTENTION_NOTIFICATIONS: &[&str] =
    &["permission_prompt", "idle_prompt", "elicitation_dialog"];

pub fn data_dir() -> PathBuf {
    if let Ok(home) = env::var("TOKENMETER_HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home);
        }
    }
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/".into());
    let home = PathBuf::from(home);
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/tokenmeter")
    } else if cfg!(windows) {
        env::var("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or(home)
            .join("tokenmeter")
    } else if let Ok(xdg) = env::var("XDG_STATE_HOME") {
        if !xdg.trim().is_empty() {
            PathBuf::from(xdg).join("tokenmeter")
        } else {
            home.join(".local/state/tokenmeter")
        }
    } else {
        home.join(".local/state/tokenmeter")
    }
}

pub fn live_dir() -> PathBuf {
    data_dir().join("live")
}

/// `ps -o command=` 한 줄이 살아 있는 네이티브 미터인지.
/// Python.app / `tokenmeter.cli daemon` / 훅 바이너리는 거부한다.
pub fn is_live_daemon_command(cmd: &str) -> bool {
    if cmd.contains("tokenmeter-hook")
        || cmd.contains("tokenmeter.cli")
        || cmd.contains("Python.app")
    {
        return false;
    }
    let cmd = cmd.trim();
    // 경로가 있든 없든(PATH 로 띄움) argv[0] 이 `tokenmeter` 인 곳 뒤의 인자.
    // 폴더 이름이 "tokenmeter 2" 처럼 겹칠 수 있어 후보를 모두 본다.
    let mut rests = ["/tokenmeter", "\\tokenmeter"]
        .iter()
        .flat_map(|b| cmd.match_indices(b).map(move |(i, _)| &cmd[i + b.len()..]))
        .chain(cmd.strip_prefix("tokenmeter"))
        .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace));
    // meter cli.rs parse 와 같게: 인자 없음·플래그로 시작·daemon·watch 는 daemon::run 을 돈다.
    rests.any(|r| {
        let mut args = r.split_whitespace();
        match args.next() {
            None | Some("daemon") => true,
            // `watch --jsonl` 은 스냅샷만 흘리고 데몬을 돌지 않는다.
            Some("watch") => !args.any(|a| a == "--jsonl"),
            Some(arg) => arg.starts_with('-') && !matches!(arg, "-h" | "--help" | "-V" | "--version"),
        }
    })
}

pub fn meter_bin() -> Option<PathBuf> {
    if let Ok(path) = env::var("TOKENMETER_BIN") {
        let path = PathBuf::from(path.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    let exe = env::current_exe().ok()?;
    let dir = exe.parent()?;
    for name in ["tokenmeter", "tokenmeter.exe"] {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

pub fn daemon_argv() -> Option<Vec<String>> {
    let bin = meter_bin()?;
    Some(vec![bin.to_string_lossy().into_owned(), "daemon".into()])
}

pub fn pick(payload: &Value, keys: &[&str]) -> String {
    let Some(obj) = payload.as_object() else {
        return String::new();
    };
    for key in keys {
        if let Some(text) = obj.get(*key).and_then(Value::as_str) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    String::new()
}

pub fn project_key(cwd: &str) -> String {
    let text = cwd.trim();
    if text.is_empty() {
        return String::new();
    }
    let path = Path::new(text);
    let leaf = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if leaf.is_empty() {
        return String::new();
    }
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("");
    if parent.is_empty() || parent == "." {
        leaf.to_string()
    } else {
        format!("{parent}/{leaf}")
    }
}

pub fn safe_name(value: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
            gap = false;
        } else if !gap {
            out.push('_');
            gap = true;
        }
    }
    if out.len() > 120 {
        out.truncate(120);
    }
    if out.is_empty() {
        "unknown".into()
    } else {
        out
    }
}

pub fn live_path(service: &str, session_id: &str) -> PathBuf {
    live_dir().join(format!(
        "{}__{}.json",
        safe_name(service),
        safe_name(session_id)
    ))
}

fn is_routing_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    upper.ends_with("_BASE_URL")
        || upper.ends_with("_API_BASE")
        || upper.ends_with("_ENDPOINT")
        || matches!(upper.as_str(), "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY")
        || upper.contains("USE_BEDROCK")
        || upper.contains("USE_VERTEX")
}

fn is_secretish(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    ["KEY", "TOKEN", "SECRET", "PASSWORD", "CREDENTIAL", "AUTH"]
        .iter()
        .any(|needle| upper.contains(needle))
}

pub fn routing_env() -> Map<String, Value> {
    let mut out = Map::new();
    for (key, value) in env::vars() {
        if value.trim().is_empty() || !is_routing_key(&key) || is_secretish(&key) {
            continue;
        }
        let clipped: String = value.chars().take(200).collect();
        out.insert(key, Value::String(clipped));
    }
    out
}

pub fn attention_signal(service: &str, event: &str, payload: &Value) -> String {
    if event == "Notification" {
        let kind = pick(payload, &["notification_type"]);
        return if ATTENTION_NOTIFICATIONS.contains(&kind.as_str()) {
            "check".into()
        } else {
            String::new()
        };
    }
    if service == "codex" && event == "PermissionRequest" {
        let reviewer =
            pick(payload, &["approvals_reviewer", "approval_reviewer"]).to_ascii_lowercase();
        if matches!(
            reviewer.as_str(),
            "auto_review" | "guardian" | "guardian_subagent"
        ) {
            return "working".into();
        }
    }
    if CHECK_EVENTS.contains(&event) {
        return "check".into();
    }
    if WAIT_EVENTS.contains(&event) {
        return "waiting".into();
    }
    if WORK_EVENTS.contains(&event) {
        "working".into()
    } else {
        String::new()
    }
}

pub fn cursor_host() -> bool {
    env::var("CURSOR_VERSION")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .is_some()
        || env::var("CURSOR_PROJECT_DIR")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .is_some()
}

/// Cursor CLI 빌드 id (`2026.09.22-161cc10`). IDE 제품 버전(`3.21.18`)은 아니다.
/// ponytail: CLI 가 semver 를 넣기 시작하면 이 구분이 깨진다.
pub fn cursor_cli_build(version: &str) -> bool {
    let version = version.trim();
    let Some((date, digest)) = version.split_once('-') else {
        return false;
    };
    if digest.is_empty() || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let mut parts = date.split('.');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    parts.next().is_none()
        && year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && [year, month, day]
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_digit()))
}

pub fn cursor_cli(payload: &Value) -> bool {
    env::var("CURSOR_VERSION")
        .ok()
        .is_some_and(|v| cursor_cli_build(&v))
        || cursor_cli_build(&pick(payload, &["cursor_version"]))
}

pub fn workspace_root(payload: &Value) -> String {
    payload
        .get("workspace_roots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

pub fn resolve_cwd(payload: &Value) -> String {
    let from_payload = pick(payload, &["cwd", "workspace", "project_dir"]);
    if !from_payload.is_empty() {
        return from_payload;
    }
    let root = workspace_root(payload);
    if !root.is_empty() {
        return root;
    }
    for key in ["TOKENMETER_CWD", "CURSOR_PROJECT_DIR", "CLAUDE_PROJECT_DIR"] {
        if let Ok(value) = env::var(key) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

pub fn resolve_session_id(payload: &Value, cwd: &str, argv_session_id: &str) -> String {
    let explicit = if argv_session_id.is_empty() {
        pick(
            payload,
            &["conversation_id", "session_id", "sessionId", "id"],
        )
    } else {
        argv_session_id.to_string()
    };
    if !explicit.is_empty() {
        return explicit;
    }
    let digest = if cfg!(target_os = "macos") {
        Command::new("md5").args(["-q", "-s", cwd]).output().ok()
    } else {
        let mut child = Command::new("md5sum")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .ok();
        if let Some(stdin) = child.as_mut().and_then(|child| child.stdin.as_mut()) {
            let _ = stdin.write_all(cwd.as_bytes());
        }
        child.and_then(|child| child.wait_with_output().ok())
    }
    .filter(|output| output.status.success())
    .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    .unwrap_or_default();
    if digest.len() >= 8 {
        return format!("cwd-{}", &digest[..8]);
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("cwd-{:08x}", hasher.finish() as u32)
}

pub fn service_for_hook(service: &str, session_id: &str) -> String {
    if service != "claude-code" {
        return service.to_string();
    }
    if cursor_host() {
        return "cursor".into();
    }
    if session_id.is_empty() {
        return service.to_string();
    }
    let grok = env::var("GROK_SESSION_ID").unwrap_or_default();
    let grok = grok.trim();
    if !grok.is_empty() && grok == session_id {
        "grok".into()
    } else {
        service.to_string()
    }
}

fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn as_f64(value: Option<&Value>) -> f64 {
    match value {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub fn write_live(
    path: &Path,
    service: &str,
    session_id: &str,
    cwd: &str,
    event: &str,
    model: &str,
    attention: &str,
) -> std::io::Result<()> {
    let existing = fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    let now = now_secs();
    let started_at = as_f64(existing.get("started_at"));
    let attention_at = as_f64(existing.get("attention_at"));
    let model = if model.is_empty() {
        pick(&Value::Object(existing.clone()), &["model"])
    } else {
        model.to_string()
    };
    let stored_attention = if attention.is_empty() {
        pick(&Value::Object(existing.clone()), &["attention"])
    } else {
        attention.to_string()
    };
    let routing = existing
        .get("routing_env")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(routing_env);
    let mut record = Map::new();
    record.insert("service".into(), json!(service));
    record.insert("session_id".into(), json!(session_id));
    record.insert("project".into(), json!(project_key(cwd)));
    record.insert("cwd".into(), json!(cwd));
    record.insert("model".into(), json!(model));
    record.insert(
        "started_at".into(),
        json!(if started_at == 0.0 { now } else { started_at }),
    );
    record.insert("event".into(), json!(event));
    record.insert("event_at".into(), json!(now));
    record.insert("routing_env".into(), Value::Object(routing));
    record.insert("attention".into(), json!(stored_attention));
    if !attention.is_empty() {
        record.insert("attention_at".into(), json!(now));
    } else if attention_at != 0.0 {
        record.insert("attention_at".into(), json!(attention_at));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        "{}.{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(Value::Object(record).to_string().as_bytes())?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

const CURSOR_USAGE_EVENTS: &[&str] = &["stop", "Stop"];

fn json_i64(value: Option<&Value>) -> Option<i64> {
    match value {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        _ => None,
    }
}

/// Cursor `stop` stdin 의 턴 누적 토큰. afterAgentResponse 는 text 가 커서 쓰지 않는다.
pub fn cursor_usage_line(event: &str, payload: &Value, session_id: &str, cwd: &str) -> Option<String> {
    if !CURSOR_USAGE_EVENTS.contains(&event) {
        return None;
    }
    let generation_id = pick(payload, &["generation_id"]);
    if generation_id.is_empty() {
        return None;
    }
    let input = json_i64(payload.get("input_tokens"));
    let output = json_i64(payload.get("output_tokens"));
    let cache_read = json_i64(payload.get("cache_read_tokens"));
    let cache_write = json_i64(payload.get("cache_write_tokens"));
    if input.is_none() && output.is_none() && cache_read.is_none() && cache_write.is_none() {
        return None;
    }
    let input = input.unwrap_or(0);
    let cache_read = cache_read.unwrap_or(0);
    let cache_write = cache_write.unwrap_or(0);
    Some(
        json!({
            "generation_id": generation_id,
            "session_id": session_id,
            "cwd": cwd,
            "model": pick(payload, &["model", "model_id"]),
            "input_tokens": (input - cache_read - cache_write).max(0),
            "cache_read_tokens": cache_read,
            "cache_write_tokens": cache_write,
            "output_tokens": output.unwrap_or(0),
        })
        .to_string(),
    )
}

pub fn append_cursor_usage(line: &str) -> io::Result<()> {
    let dir = data_dir().join("cursor");
    fs::create_dir_all(&dir)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("usage.jsonl"))?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

pub fn is_off(service: &str) -> bool {
    let text = match fs::read_to_string(data_dir().join("toggle.json")) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let Ok(data) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let Some(obj) = data.as_object() else {
        return false;
    };
    if obj.get("enabled") == Some(&Value::Bool(false)) {
        return true;
    }
    obj.get("services")
        .and_then(Value::as_object)
        .and_then(|m| m.get(service))
        == Some(&Value::Bool(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn attention_signal_matches_python_contract() {
        let empty = json!({});
        assert_eq!(attention_signal("claude-code", "Stop", &empty), "waiting");
        assert_eq!(
            attention_signal("opencode", "session.idle", &empty),
            "waiting"
        );
        assert_eq!(
            attention_signal(
                "claude-code",
                "Notification",
                &json!({"notification_type": "auth_success"})
            ),
            ""
        );
        assert_eq!(
            attention_signal(
                "claude-code",
                "Notification",
                &json!({"notification_type": "permission_prompt"})
            ),
            "check"
        );
        assert_eq!(
            attention_signal(
                "codex",
                "PermissionRequest",
                &json!({"approvals_reviewer": "auto_review"})
            ),
            "working"
        );
        assert_eq!(
            attention_signal("opencode", "question.asked", &empty),
            "check"
        );
        assert_eq!(
            attention_signal("claude-code", "SessionStart", &empty),
            "working"
        );
        assert_eq!(attention_signal("claude-code", "unknown", &empty), "");
    }

    #[test]
    fn project_key_keeps_parent_leaf() {
        assert_eq!(project_key("/Users/dev/work/api"), "work/api");
        assert_eq!(project_key("/Users/dev/work/acme/api"), "acme/api");
        assert_eq!(project_key("/foo"), "foo");
        assert_eq!(project_key("/"), "");
        assert_eq!(project_key(""), "");
    }

    #[test]
    fn safe_name_collapses_unsafe_runs() {
        assert_eq!(safe_name("a  b/c"), "a_b_c");
        assert_eq!(safe_name(""), "unknown");
    }

    #[test]
    fn cwd_fallback_matches_python_md5() {
        assert_eq!(
            resolve_session_id(&json!({}), "/tmp/demo", ""),
            "cwd-b9375f22"
        );
    }

    #[test]
    fn service_for_hook_requires_matching_grok_session() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        env::remove_var("GROK_SESSION_ID");
        env::remove_var("CURSOR_VERSION");
        env::remove_var("CURSOR_PROJECT_DIR");
        assert_eq!(service_for_hook("claude-code", "g-1"), "claude-code");
        env::set_var("GROK_SESSION_ID", "g-other");
        assert_eq!(service_for_hook("claude-code", "g-1"), "claude-code");
        env::set_var("GROK_SESSION_ID", "g-1");
        assert_eq!(service_for_hook("claude-code", "g-1"), "grok");
        env::remove_var("GROK_SESSION_ID");
    }

    #[test]
    fn cursor_cli_build_rejects_ide_version() {
        assert!(cursor_cli_build("2026.09.22-161cc10"));
        assert!(!cursor_cli_build("3.21.18"));
        assert!(!cursor_cli_build("2.0.0"));
        assert!(!cursor_cli_build(""));
    }

    #[test]
    fn service_for_hook_remaps_cursor_host() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        env::remove_var("GROK_SESSION_ID");
        env::remove_var("CURSOR_PROJECT_DIR");
        env::set_var("CURSOR_VERSION", "2.0.0");
        assert_eq!(service_for_hook("claude-code", "any"), "cursor");
        env::remove_var("CURSOR_VERSION");
        assert_eq!(service_for_hook("claude-code", "any"), "claude-code");
    }

    #[test]
    fn cursor_payload_fields() {
        assert_eq!(
            resolve_session_id(&json!({"conversation_id": "conv-1"}), "/tmp", ""),
            "conv-1"
        );
        assert_eq!(
            workspace_root(&json!({"workspace_roots": ["/work/token-pet", "/other"]})),
            "/work/token-pet"
        );
        assert_eq!(
            attention_signal("cursor", "sessionStart", &json!({})),
            "working"
        );
        assert_eq!(
            attention_signal("cursor", "afterAgentThought", &json!({})),
            "working"
        );
        assert_eq!(
            attention_signal("cursor", "postToolUse", &json!({})),
            "working"
        );
        assert_eq!(attention_signal("cursor", "stop", &json!({})), "waiting");
        assert!(STOP_EVENTS.contains(&"sessionEnd"));
        let payload = json!({
            "generation_id": "g1",
            "input_tokens": 100,
            "output_tokens": 7,
            "cache_read_tokens": 80,
            "cache_write_tokens": 5,
            "model": "cursor-grok-4.6",
        });
        let line = cursor_usage_line("stop", &payload, "s1", "/work").unwrap();
        let rec: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(rec["input_tokens"], 15);
        assert_eq!(rec["output_tokens"], 7);
        assert_eq!(rec["session_id"], "s1");
        assert!(cursor_usage_line("afterAgentResponse", &payload, "s1", "/work").is_none());
        assert!(cursor_usage_line("stop", &json!({"generation_id": "g1"}), "s1", "").is_none());
    }

    #[test]
    fn routing_env_keeps_endpoints_and_drops_secrets() {
        let _guard = ENV_LOCK.lock().expect("env lock");
        let vars = [
            ("ANTHROPIC_BASE_URL", "https://llm.mycorp.com/v1"),
            ("ANTHROPIC_API_KEY", "present"),
            ("ANTHROPIC_AUTH_TOKEN", "present"),
            ("HTTPS_PROXY", "http://proxy.corp:3128"),
            ("EDITOR", "vim"),
        ];
        for (key, value) in vars {
            env::set_var(key, value);
        }
        let routing = routing_env();
        for (key, _) in vars {
            env::remove_var(key);
        }
        assert_eq!(routing["ANTHROPIC_BASE_URL"], "https://llm.mycorp.com/v1");
        assert_eq!(routing["HTTPS_PROXY"], "http://proxy.corp:3128");
        for key in ["EDITOR", "ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN"] {
            assert!(!routing.contains_key(key), "{key}");
        }
    }

    #[test]
    fn live_record_stores_allowlisted_fields_only() {
        let path = env::temp_dir().join(format!("tokenmeter-hook-live-{}.json", std::process::id()));
        let secret = json!({"tool_input": {"command": "secret-command"}});
        let attention = attention_signal("codex", "PermissionRequest", &secret);
        write_live(&path, "codex", "s", "/work/api", "PermissionRequest", "gpt", &attention).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);
        assert!(!text.contains("secret-command"));
        let stored: Value = serde_json::from_str(&text).unwrap();
        let allowed = [
            "service", "session_id", "project", "cwd", "model", "started_at",
            "event", "event_at", "routing_env", "attention", "attention_at",
        ];
        for key in stored.as_object().unwrap().keys() {
            assert!(allowed.contains(&key.as_str()), "{key}");
        }
    }

    #[test]
    fn live_daemon_command_accepts_native_rejects_python() {
        assert!(is_live_daemon_command(
            "/Users/x/tokenmeter/bin/tokenmeter daemon --no-window"
        ));
        assert!(is_live_daemon_command("tokenmeter daemon --no-window"), "PATH 로 띄운 데몬");
        // 인자 없음·플래그로 시작·watch 도 daemon::run 을 돈다 (meter cli.rs parse, cmd_watch).
        for cmd in [
            "tokenmeter",
            "/x/tokenmeter\n",
            "tokenmeter --no-window",
            "/x/tokenmeter watch",
            "/x/tokenmeter 2/bin/tokenmeter daemon",
        ] {
            assert!(is_live_daemon_command(cmd), "{cmd}");
        }
        for cmd in [
            "tokenmeter status",
            "/x/tokenmeter --help",
            "/x/tokenmeter -V",
            "/x/tokenmeters daemon",
            "/x/tokenmeter 2/bin/tokenmeter status",
            "tokenmeter watch --jsonl",
        ] {
            assert!(!is_live_daemon_command(cmd), "{cmd}");
        }
        assert!(!is_live_daemon_command(
            "/opt/homebrew/Cellar/python@3.14/Resources/Python.app/Contents/MacOS/Python -m tokenmeter.cli daemon"
        ));
        assert!(!is_live_daemon_command(
            "/Users/x/tokenmeter/bin/tokenmeter-hook claude-code SessionStart"
        ));
    }
}
