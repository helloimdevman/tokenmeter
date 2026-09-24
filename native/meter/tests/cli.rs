//! 네이티브 `tokenmeter` 를 임시 HOME 에서 실제로 돌린다. 실제 사용자 설정은 건드리지 않는다.

use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_tokenmeter");

fn sandbox(tag: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("cli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("home")).unwrap();
    root
}

fn command(root: &Path, bin: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", root.join("home"))
        .env("TOKENMETER_HOME", root.join("state"))
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("xdg-state"))
        .env("TOKENMETER_NO_DAEMON", "1")
        .env("TOKENMETER_NO_PROMPT", "1")
        .current_dir(root)
        .stdin(Stdio::null());
    cmd
}

fn run(root: &Path, bin: &Path, args: &[&str]) -> Output {
    command(root, bin, args).output().unwrap()
}

fn tm(root: &Path, args: &[&str]) -> Output {
    run(root, Path::new(BIN), args)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn help_and_version_name_the_native_cli() {
    let root = sandbox("help");
    let help = tm(&root, &["--help"]);
    assert!(help.status.success());
    let text = stdout(&help);
    assert!(text.contains("usage: tokenmeter") && text.contains("TokenMeter"), "{text}");
    for cmd in ["install", "uninstall", "update", "doctor", "status"] {
        assert!(text.contains(cmd), "{cmd}");
    }
    assert_eq!(stdout(&tm(&root, &["--version"])).trim(), format!("tokenmeter {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn status_json_is_one_clean_public_object() {
    let root = sandbox("status");
    write(
        &root.join("state/state.json"),
        &json!({"updated_at": {}, "sessions": ["broken"],
                "today": {"date": "2026-08-14", "totals": ["broken"]}, "total": ["broken"]})
        .to_string(),
    );
    let out = tm(&root, &["status", "--json"]);
    assert!(out.status.success() && out.stderr.is_empty(), "{}", String::from_utf8_lossy(&out.stderr));
    let snap: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!((snap["type"].clone(), snap["sessions"].clone()), (json!("snapshot"), json!([])));
    assert_eq!(snap["today"]["totals"]["input_tokens"], 0);

    let home = root.join("home").display().to_string();
    let cwd = format!("{home}/dev/api");
    write(
        &root.join("state/state.json"),
        &json!({"sessions": {"claude-code/sess-secret-1": {
            "service": "claude-code", "project": "dev/api", "cwd": cwd, "prompt": "DO-NOT-LEAK",
            "model": "claude-opus-5", "last_seen": 1.0, "totals": {"output_tokens": 5}}},
            "endpoints": {"https://llm.corp.example/v1": {"totals": {"output_tokens": 5}}}})
        .to_string(),
    );
    write(
        &root.join("state/live/claude-code__sess-secret-1.json"),
        &json!({"service": "claude-code", "session_id": "sess-secret-1", "cwd": cwd,
                "routing_env": {"ANTHROPIC_BASE_URL": "https://llm.corp.example/v1"},
                "attention": "working", "attention_at": 1.0})
        .to_string(),
    );
    let raw = stdout(&tm(&root, &["status", "--json"]));
    let snap: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(snap["sessions"].as_array().unwrap().len(), 1);
    for private in [home.as_str(), "sess-secret-1", "DO-NOT-LEAK", "llm.corp.example", "routing_env"] {
        assert!(!raw.contains(private), "{private} 가 status --json 에 샜다");
    }
}

#[test]
fn doctor_json_omits_home_paths_session_ids_and_prompts() {
    let root = sandbox("doctor");
    let home = root.join("home").display().to_string();
    let session = "0b6f1c2a-4d5e-4f60-8a7b-9c0d1e2f3a4b";
    let record = json!({
        "type": "assistant", "uuid": "u-1", "cwd": format!("{home}/dev/api"), "sessionId": session,
        "message": {"model": "claude-opus-5", "content": "DO-NOT-LEAK prompt text",
                    "usage": {"input_tokens": 2, "output_tokens": 8}}
    });
    write(
        &root.join(format!("home/.claude/projects/-Users-alice-dev-api/{session}.jsonl")),
        &format!("{record}\n"),
    );
    let out = tm(&root, &["doctor", "--json"]);
    assert!(out.status.success());
    let raw = stdout(&out);
    let payload: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(payload["version"], env!("CARGO_PKG_VERSION"));
    assert!(payload["os"].as_str().is_some() && payload.get("native_meter").is_some());
    for private in [home.as_str(), session, "DO-NOT-LEAK", "prompt", "-Users-alice"] {
        assert!(!raw.contains(private), "{private} 가 doctor --json 에 샜다: {raw}");
    }
}

/// 실제 바이너리를 임시 bin/ 에 복사하고 가짜 훅을 옆에 둔다 — install 이 GitHub 에서 훅을 받지 않게.
fn installed_copy(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let meter = bin.join("tokenmeter");
    if fs::hard_link(BIN, &meter).is_err() {
        fs::copy(BIN, &meter).unwrap();
    }
    write(&bin.join("tokenmeter-hook"), "#!/bin/sh\nexit 0\n");
    meter
}

#[test]
fn install_twice_then_uninstall_restores_agent_configs() {
    let root = sandbox("install");
    let meter = installed_copy(&root);
    let hook = root.join("bin/tokenmeter-hook").display().to_string();
    let home = root.join("home");
    let claude_path = home.join(".claude/settings.json");
    let codex_path = home.join(".codex/hooks.json");
    let claude = json!({
        "model": "opus",
        "hooks": {"SessionStart": [{"matcher": "", "hooks": [{"type": "command", "command": "/usr/bin/other-hook.sh"}]}]}
    });
    let codex = json!({"hooks": {"Stop": [{"matcher": "", "hooks": [{"type": "command", "command": "notify-send done"}]}]}});
    write(&claude_path, &serde_json::to_string_pretty(&claude).unwrap());
    write(&codex_path, &serde_json::to_string_pretty(&codex).unwrap());

    for _ in 0..2 {
        let out = run(&root, &meter, &["install"]);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let text = stdout(&out);
        assert!(text.contains("완전히 다시") && text.contains("tokenmeter doctor"), "첫 측정 안내가 없다: {text}");
    }

    let specs = tokenmeter::watch::default_specs();
    for (name, path) in [("claude-code", &claude_path), ("codex", &codex_path)] {
        let spec = specs.iter().find(|s| s.name == name).unwrap();
        let data = read(path);
        for event in &spec.install.events {
            let wanted = format!("\"{hook}\" {name} {event}");
            let count = data["hooks"][event].as_array().unwrap().iter()
                .flat_map(|g| g["hooks"].as_array().unwrap().iter())
                .filter(|e| e["command"] == wanted.as_str())
                .count();
            assert_eq!(count, 1, "{name} {event}");
        }
        let backup = format!("{}.bak-tokenmeter", path.file_name().unwrap().to_string_lossy());
        assert!(path.with_file_name(backup).exists(), "{name} 백업이 없다");
    }
    assert_eq!(read(&claude_path)["model"], "opus");
    let plugin = home.join(".config/opencode/plugin/tokenmeter.js");
    assert!(fs::read_to_string(&plugin).unwrap().contains("tokenmeter:generated"));
    assert_eq!(read(&home.join(".cursor/hooks.json"))["version"], 1);
    assert!(fs::read_to_string(home.join(".grok/hooks/tokenmeter.json")).unwrap().contains(&hook));

    let status = stdout(&run(&root, &meter, &["status"]));
    assert!(status.contains("첫 세션 대기 중") && status.contains("tokenmeter doctor"), "{status}");

    assert!(run(&root, &meter, &["uninstall"]).status.success());
    assert_eq!(read(&claude_path), claude, "해제 후 원본과 달라졌다");
    assert_eq!(read(&codex_path), codex);
    assert!(!plugin.exists());
    for path in [home.join(".cursor/hooks.json"), home.join(".grok/hooks/tokenmeter.json")] {
        assert!(!fs::read_to_string(&path).unwrap().contains("tokenmeter-hook"), "{}", path.display());
    }
}

#[test]
fn adapter_cli_reports_check_result_and_rejects_escaping_names() {
    let root = sandbox("adapter");
    let log = root.join("agent.jsonl");
    write(&log, "{\"usage\": {\"input_tokens\": 1, \"output_tokens\": 2}, \"model\": \"m\", \"session_id\": \"s\"}\n");
    let log = log.display().to_string();
    assert!(tm(&root, &["adapter", "init", "sample", "--log", &log]).status.success());
    let yaml = root.join("sample-adapter/service.yaml");
    let text = fs::read_to_string(&yaml).unwrap();
    fs::write(&yaml, text.replace("mode: choose-delta-or-cumulative", "mode: cumulative")).unwrap();
    let out = tm(&root, &["adapter", "check", "sample-adapter"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert_eq!(
        stdout(&out).trim(),
        "sample: 구조 검증 통과 (연결된 토큰 필드: input=usage.input_tokens, output=usage.output_tokens)"
    );
    for name in ["../outside", "/absolute"] {
        assert_eq!(tm(&root, &["adapter", "init", name, "--log", &log]).status.code(), Some(1), "{name}");
    }
    assert!(!root.parent().unwrap().join("outside-adapter").exists());
    assert!(!Path::new("/absolute-adapter").exists());
}

fn receipt_state(root: &Path) {
    write(
        &root.join("state/state.json"),
        &json!({"sessions": {
            "claude-code/old": {"project": "old", "last_seen": 1, "totals": {"cost_usd": 9}},
            "codex/new": {
                "service": "codex", "project": "api", "model": "gpt-5.6-sol", "effort": "high",
                "plan": "subscription", "started_at": 10, "last_seen": 70,
                "ctx": 50_000, "ctx_win": 200_000, "sub_cost": 1.0,
                "totals": {"input_tokens": 10, "cache_read": 20, "cache_write": 3, "output_tokens": 7,
                           "cost_usd": 4.0, "cache_saved_usd": 2.0, "calls": 2}
            }
        }})
        .to_string(),
    );
}

#[test]
fn receipt_without_sessions_exits_1() {
    let root = sandbox("receipt-empty");
    write(&root.join("state/state.json"), r#"{"sessions": {}}"#);
    let out = tm(&root, &["receipt", "--format", "text"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout(&out).trim(), "영수증을 만들 세션이 없습니다.");
}

#[test]
#[ignore = "Rust receipt 는 3줄(JSON 따옴표 포함)만 낸다 — 금액 라벨·시간·캐시·ctx/서브에이전트 줄과 markdown 형식이 없다 (docs/reference.ko.md 계약)"]
fn receipt_formats_follow_reference() {
    let root = sandbox("receipt");
    receipt_state(&root);
    assert_eq!(
        stdout(&tm(&root, &["receipt", "--format", "text"])).trim_end(),
        [
            "TokenMeter 영수증",
            "api · codex · gpt-5.6-sol",
            "1분 · 입력 10 · 캐시 읽기 20 · 캐시 쓰기 3 · 출력 7 · 2 호출",
            "API 환산 가치 $4.00 · 캐시 절감 $2.00",
            "ctx 25% · 서브에이전트 25%",
        ]
        .join("\n")
    );
    assert_eq!(
        stdout(&tm(&root, &["receipt", "--format", "markdown"])).trim_end(),
        [
            "### TokenMeter 영수증",
            "- api · codex · gpt-5.6-sol",
            "- 1분 · 입력 10 · 캐시 읽기 20 · 캐시 쓰기 3 · 출력 7 · 2 호출",
            "- API 환산 가치 $4.00 · 캐시 절감 $2.00",
            "- ctx 25% · 서브에이전트 25%",
        ]
        .join("\n")
    );
}

#[test]
#[ignore = "Rust watch --jsonl 은 변경마다 snapshot 만 다시 낸다 — delta/attention 레코드가 없다 (docs/reference.ko.md 계약)"]
fn watch_jsonl_emits_delta_after_first_snapshot() {
    let root = sandbox("watch");
    let totals = |n: i64| json!({"input_tokens": 0, "cache_read": 0, "cache_write": 0, "output_tokens": n, "cost_usd": 0.0, "calls": n});
    let state = |at: f64, n: i64| json!({"updated_at": at, "today": {"date": "2026-08-14", "totals": totals(n)}, "total": {"totals": totals(n)}}).to_string();
    write(&root.join("state/state.json"), &state(1.0, 1));
    let mut child = command(&root, Path::new(BIN), &["watch", "--jsonl"]).stdout(Stdio::piped()).spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let first: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    write(&root.join("state/state.json"), &state(2.0, 3));
    let second: Value = serde_json::from_str(&lines.next().unwrap().unwrap()).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(first["type"], "snapshot");
    assert_eq!(second["type"], "delta");
    assert_eq!(second["delta"], json!({"output_tokens": 2, "calls": 2}));
}

#[test]
#[ignore = "Rust println! 은 닫힌 stdout(EPIPE)에서 panic 하고 101 로 끝난다 — `tokenmeter price | head -1` 이 stderr 를 더럽힌다"]
fn closed_stdout_is_a_clean_exit() {
    let root = sandbox("pipe");
    let (reader, writer) = std::io::pipe().unwrap();
    drop(reader);
    let out = command(&root, Path::new(BIN), &["price"]).stdout(writer).stderr(Stdio::piped()).output().unwrap();
    assert!(out.status.success(), "exit {:?}: {}", out.status.code(), String::from_utf8_lossy(&out.stderr));
    assert!(out.stderr.is_empty());
}
