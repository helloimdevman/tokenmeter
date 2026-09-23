//! 에이전트 훅 엔트리. stdout 금지, 어떤 입력이든 exit 0.

use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, IsTerminal, Read, Write};

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime};
use tokenmeter_hook::{
    attention_signal, cursor_cli, cursor_host, data_dir, is_off, live_path, pick, resolve_cwd,
    append_cursor_usage, cursor_usage_line, resolve_session_id, service_for_hook, write_live,
    STOP_EVENTS,
};

const LOCK_STALE_SECS: u64 = 60;
const STDIN_TIMEOUT_MS: u64 = 200;

fn debug(msg: &str) {
    if env::var("TOKENMETER_DEBUG").ok().as_deref() == Some("1") {
        eprintln!("[tokenmeter-hook] {msg}");
    }
}

fn read_payload() -> Value {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return json!({});
    }
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut raw = String::new();
        let _ = io::stdin().read_to_string(&mut raw);
        let _ = tx.send(raw);
    });
    let raw = match rx.recv_timeout(Duration::from_millis(STDIN_TIMEOUT_MS)) {
        Ok(text) => text,
        Err(_) => return json!({}),
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return json!({});
    }
    serde_json::from_str::<Value>(trimmed)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}))
}

fn daemon_pid() -> i32 {
    let text = match fs::read_to_string(data_dir().join("tokenmeter.pid")) {
        Ok(t) => t,
        Err(_) => return 0,
    };
    let pid: i32 = match text.trim().parse() {
        Ok(n) if n > 0 => n,
        _ => return 0,
    };
    let output = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output();
    match output {
        Ok(out) => {
            let cmd = String::from_utf8_lossy(&out.stdout);
            if tokenmeter_hook::is_live_daemon_command(&cmd) {
                pid
            } else {
                0
            }
        }
        Err(_) => pid,
    }
}

fn take_lock() -> bool {
    let lock = data_dir().join("daemon.lock");
    let _ = fs::create_dir_all(data_dir());
    for _ in 0..2 {
        match OpenOptions::new().write(true).create_new(true).open(&lock) {
            Ok(mut file) => {
                let _ = write!(file, "{}", std::process::id());
                return true;
            }
            Err(_) => {
                let age = fs::metadata(&lock)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| SystemTime::now().duration_since(t).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                if age < LOCK_STALE_SECS {
                    return false;
                }
                if fs::remove_file(&lock).is_err() {
                    return false;
                }
            }
        }
    }
    false
}

fn ensure_daemon() -> bool {
    if env::var("TOKENMETER_NO_DAEMON").ok().as_deref() == Some("1") {
        return false;
    }
    if daemon_pid() != 0 {
        return false;
    }
    let Some(argv) = tokenmeter_hook::daemon_argv() else {
        debug("네이티브 미터 바이너리가 없어 데몬을 띄우지 않습니다");
        return false;
    };
    if !take_lock() {
        return false;
    }
    let log_path = data_dir().join("daemon.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();
    let mut cmd = Command::new(&argv[0]);
    if argv.len() > 1 {
        cmd.args(&argv[1..]);
    }
    cmd.stdin(Stdio::null());
    if let Some(file) = log {
        match file.try_clone() {
            Ok(err) => {
                cmd.stdout(Stdio::from(file));
                cmd.stderr(Stdio::from(err));
            }
            Err(_) => {
                cmd.stdout(Stdio::null());
                cmd.stderr(Stdio::null());
            }
        }
    } else {
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    match cmd.spawn() {
        Ok(_) => true,
        Err(err) => {
            debug(&format!("데몬 기동 실패: {err}"));
            false
        }
    }
}

fn run(args: &[String]) -> i32 {
    if env::var("TOKENMETER_DISABLE").ok().as_deref() == Some("1") {
        return 0;
    }
    let mut service = args
        .first()
        .map(String::as_str)
        .unwrap_or("unknown")
        .to_string();
    let mut event = args.get(1).cloned().unwrap_or_default();
    let argv_sid = args.get(2).cloned().unwrap_or_default();
    let payload = read_payload();
    if event.is_empty() {
        event = pick(&payload, &["hook_event_name"]);
        if event.is_empty() {
            event = "SessionStart".into();
        }
    }
    let cwd = resolve_cwd(&payload);
    let session_id = resolve_session_id(&payload, &cwd, &argv_sid);
    service = service_for_hook(&service, &session_id);
    if service == "cursor" && cursor_cli(&payload) {
        let _ = fs::remove_file(live_path(&service, &session_id));
        return 0;
    }
    if is_off(&service) {
        return 0;
    }
    if let Some(line) = cursor_usage_line(&event, &payload, &session_id, &cwd) {
        if let Err(err) = append_cursor_usage(&line) {
            debug(&format!("커서 사용량 기록 실패: {err}"));
        }
    }
    let path = live_path(&service, &session_id);
    if STOP_EVENTS.contains(&event.as_str()) {
        let _ = fs::remove_file(&path);
    } else {
        let model = pick(&payload, &["model", "model_id"]);
        let attention = attention_signal(&service, &event, &payload);
        if let Err(err) = write_live(
            &path,
            &service,
            &session_id,
            &cwd,
            &event,
            &model,
            &attention,
        ) {
            debug(&format!("라이브 기록 실패: {err}"));
        }
        ensure_daemon();
    }
    debug(&format!(
        "{service}/{event} sid={session_id} → {}",
        path.display()
    ));
    0
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let ack = cursor_host() || args.iter().any(|a| a == "cursor");
    let code = std::panic::catch_unwind(|| run(&args)).unwrap_or(0);
    if ack {
        println!("{{\"continue\":true}}");
    }
    std::process::exit(code);
}
