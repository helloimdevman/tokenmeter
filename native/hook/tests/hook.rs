//! 네이티브 `tokenmeter-hook` 을 에이전트처럼 띄운다. 상태는 임시 디렉터리에만 쓴다.

use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

fn sandbox(tag: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("hook-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

fn hook(root: &Path, args: &[&str], stdin: &str, env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tokenmeter-hook"));
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", root.join("home"))
        .env("TOKENMETER_HOME", root.join("state"))
        .env("TOKENMETER_CWD", "/Users/dev/projects/tokenmeter")
        .env("TOKENMETER_NO_DAEMON", "1");
    for (key, value) in env {
        cmd.env(key, value);
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{args:?}: 훅은 무슨 일이 있어도 exit 0");
    out
}

fn live(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(root.join("state/live"))
        .map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    names.retain(|n| n.ends_with(".json"));
    names.sort();
    names
}

fn record(root: &Path, name: &str) -> Value {
    serde_json::from_str(&fs::read_to_string(root.join("state/live").join(name)).unwrap()).unwrap()
}

#[test]
fn session_start_writes_live_file_silently_and_session_end_removes_it() {
    let root = sandbox("life");
    let started = hook(&root, &["claude-code", "SessionStart", "s1"], "", &[]);
    assert!(started.stdout.is_empty(), "SessionStart stdout 은 컨텍스트로 주입된다 — 비어야 한다");
    assert_eq!(live(&root), ["claude-code__s1.json"]);
    let rec = record(&root, "claude-code__s1.json");
    assert_eq!(
        (rec["service"].clone(), rec["session_id"].clone(), rec["project"].clone(), rec["event"].clone(), rec["attention"].clone()),
        (json!("claude-code"), json!("s1"), json!("projects/tokenmeter"), json!("SessionStart"), json!("working"))
    );

    hook(&root, &["claude-code", "Ping", "s1"], "", &[]);
    assert_eq!(live(&root), ["claude-code__s1.json"], "중간 이벤트는 파일을 늘리지 않는다");
    hook(&root, &["claude-code", "Stop", "s1"], "", &[]);
    assert_eq!(record(&root, "claude-code__s1.json")["attention"], "waiting", "Stop 은 턴 완료이지 세션 종료가 아니다");
    hook(&root, &["claude-code", "SessionEnd", "s1"], "", &[]);
    assert!(live(&root).is_empty());

    hook(&root, &["claude-code", "SessionStart", "claude-1"], "", &[("GROK_SESSION_ID", "g-other")]);
    assert_eq!(live(&root), ["claude-code__claude-1.json"], "셸에 남은 GROK_SESSION_ID 만으로 가로채면 안 된다");
    hook(&root, &["claude-code", "SessionStart", "g-1"], "", &[("GROK_SESSION_ID", "g-1")]);
    assert!(live(&root).contains(&"grok__g-1.json".to_string()), "Grok compat 훅은 같은 세션 id 일 때만 옮긴다");

    hook(&root, &["claude-code", "SessionStart", "quiet"], "", &[("TOKENMETER_DISABLE", "1")]);
    assert!(!live(&root).contains(&"claude-code__quiet.json".to_string()), "TOKENMETER_DISABLE=1 이면 아무것도 안 한다");
}

#[test]
fn off_toggle_makes_the_hook_exit_early() {
    let root = sandbox("off");
    let toggle = root.join("state/toggle.json");
    fs::create_dir_all(toggle.parent().unwrap()).unwrap();
    fs::write(&toggle, r#"{"enabled": false}"#).unwrap();
    hook(&root, &["claude-code", "SessionStart", "a"], "", &[]);
    assert!(live(&root).is_empty(), "전체를 끄면 훅이 바로 물러난다");

    fs::write(&toggle, r#"{"services": {"codex": false}}"#).unwrap();
    hook(&root, &["codex", "SessionStart", "b"], "", &[]);
    hook(&root, &["claude-code", "SessionStart", "c"], "", &[]);
    assert_eq!(live(&root), ["claude-code__c.json"], "하나만 끄면 나머지는 잰다");

    fs::write(&toggle, "{망가짐").unwrap();
    hook(&root, &["codex", "SessionStart", "d"], "", &[]);
    assert!(live(&root).contains(&"codex__d.json".to_string()), "깨진 토글은 켜진 것으로 본다");
}

#[test]
fn cursor_payload_acks_and_records_turn_usage_for_the_ide_only() {
    let root = sandbox("cursor");
    let usage = root.join("state/cursor/usage.jsonl");
    let payload = |extra: Value| {
        let mut base = json!({"conversation_id": "conv-1", "workspace_roots": ["/Users/dev/work/token-pet"], "model": "grok-4.6"});
        base.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        base.to_string()
    };
    let ack = |out: &Output| serde_json::from_slice::<Value>(&out.stdout).unwrap();

    let out = hook(&root, &["cursor", "sessionStart"], &payload(json!({})), &[]);
    assert_eq!(ack(&out), json!({"continue": true}));
    assert_eq!(live(&root), ["cursor__conv-1.json"]);
    let rec = record(&root, "cursor__conv-1.json");
    assert_eq!(
        (rec["project"].clone(), rec["model"].clone(), rec["attention"].clone()),
        (json!("work/token-pet"), json!("grok-4.6"), json!("working"))
    );

    hook(&root, &["cursor", "stop", "conv-1"], &payload(json!({})), &[]);
    assert!(!usage.exists(), "generation_id 없는 stop 은 사용량이 아니다");
    hook(&root, &["cursor", "stop", "conv-1"], &payload(json!({
        "generation_id": "g1", "input_tokens": 100, "output_tokens": 7,
        "cache_read_tokens": 80, "cache_write_tokens": 5
    })), &[]);
    let row: Value = serde_json::from_str(fs::read_to_string(&usage).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!((row["input_tokens"].clone(), row["output_tokens"].clone(), row["generation_id"].clone()),
               (json!(15), json!(7), json!("g1")), "입력은 캐시를 뺀다");
    hook(&root, &["cursor", "sessionEnd", "conv-1"], &payload(json!({})), &[]);
    assert!(live(&root).is_empty());

    let out = hook(&root, &["claude-code", "SessionStart"], &payload(json!({})), &[("CURSOR_VERSION", "2.0.0")]);
    assert_eq!(ack(&out), json!({"continue": true}), "Cursor 가 부른 Claude 훅도 ack 한다");
    assert_eq!(live(&root), ["cursor__conv-1.json"]);

    hook(&root, &["cursor", "stop", "conv-1"], &payload(json!({
        "cursor_version": "2026.09.22-161cc10", "generation_id": "g-cli", "input_tokens": 9, "output_tokens": 1
    })), &[("CURSOR_VERSION", "2026.09.22-161cc10")]);
    assert!(live(&root).is_empty(), "Cursor CLI 세션은 재지 않는다");
    let rows = fs::read_to_string(&usage).unwrap();
    assert!(rows.lines().count() == 1 && !rows.contains("g-cli"));
}

#[test]
fn session_start_spawns_the_native_meter_daemon() {
    let root = sandbox("daemon");
    let fake = root.join("bin/tokenmeter");
    fs::create_dir_all(fake.parent().unwrap()).unwrap();
    fs::write(&fake, "#!/bin/sh\necho \"$@\" > \"$(dirname \"$0\")/argv\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let fake = fake.display().to_string();
    hook(&root, &["claude-code", "SessionStart", "s"], "", &[("TOKENMETER_BIN", &fake), ("TOKENMETER_NO_DAEMON", "0")]);
    let argv = root.join("bin/argv");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !argv.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fs::read_to_string(&argv).unwrap_or_default().trim(), "daemon", "파이썬이 아니라 네이티브 미터를 띄운다");
}
