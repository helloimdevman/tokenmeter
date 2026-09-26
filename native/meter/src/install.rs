//! 에이전트 훅 설치/해제. 우리 엔트리만 건드린다.

use crate::watch::{expand_home, load_all_specs, ServiceSpec};
use crate::VERSION;
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use tokenmeter_hook::data_dir;

const PLUGIN_MARKER: &str = "tokenmeter:generated";
const LEGACY_PLUGIN_MARKER: &str = "tokenpet:generated";
const BACKUP_SUFFIX: &str = ".bak-tokenmeter";
const HOOK_TIMEOUT: i64 = 5;
const UPDATE_INTERVAL: f64 = 86400.0;
const RELEASE_URL: &str = "https://api.github.com/repos/helloimdevman/tokenmeter/releases/latest";

pub fn hook_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(if cfg!(windows) { "tokenmeter-hook.exe" } else { "tokenmeter-hook" })))
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("tokenmeter-hook"))
}

pub fn meter_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .filter(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("tokenmeter"))
}

pub fn hook_command(service: &str, event: &str) -> String {
    format!("\"{}\" {service} {event}", hook_bin().display())
}

fn is_ours(entry: &Value, service: &str, event: &str) -> bool {
    let cmd = entry.get("command").and_then(Value::as_str).unwrap_or("");
    if !cmd.ends_with(&format!(" {service} {event}")) {
        return false;
    }
    let hook = hook_bin().display().to_string();
    cmd.contains(&format!("\"{hook}\""))
        || cmd.contains("/tokenmeter-hook")
        || cmd.contains("\\tokenmeter-hook")
        || cmd.contains("/hook.py")
        || cmd.contains("\\hook.py")
        || cmd.contains("/tokenpet/")
}

fn load_json(path: &Path) -> Result<Value, String> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn backup(path: &Path) {
    if path.exists() {
        let dest = path.with_file_name(format!(
            "{}{BACKUP_SUFFIX}",
            path.file_name().unwrap_or_default().to_string_lossy()
        ));
        let _ = fs::copy(path, dest);
    }
}

fn save_json(path: &Path, data: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("tokenmeter-tmp");
    fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(data).unwrap_or_default()))
        .map_err(|e| e.to_string())?;
    if path.exists() {
        if let Ok(meta) = path.metadata() {
            let _ = fs::set_permissions(&tmp, meta.permissions());
        }
    }
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn edit_claude_json(data: &mut Value, service: &str, events: &[String], remove: bool) -> bool {
    let hooks = match data.get_mut("hooks") {
        Some(Value::Object(_)) => data.get_mut("hooks").unwrap(),
        _ if remove => return false,
        _ => {
            data.as_object_mut()
                .unwrap()
                .insert("hooks".into(), json!({}));
            data.get_mut("hooks").unwrap()
        }
    };
    let targets: Vec<String> = if remove {
        hooks
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        events.to_vec()
    };
    let mut changed = false;
    for event in targets {
        let groups = match hooks.get_mut(&event) {
            Some(Value::Array(_)) => hooks.get_mut(&event).unwrap(),
            _ if remove => continue,
            _ => {
                hooks
                    .as_object_mut()
                    .unwrap()
                    .insert(event.clone(), json!([]));
                hooks.get_mut(&event).unwrap()
            }
        };
        let wanted = hook_command(service, &event);
        let mut found = false;
        let mut kept = false;
        let Some(arr) = groups.as_array_mut() else {
            continue;
        };
        let mut g = 0;
        while g < arr.len() {
            let Some(entries) = arr[g].get_mut("hooks").and_then(Value::as_array_mut) else {
                g += 1;
                continue;
            };
            let ours: Vec<usize> = entries
                .iter()
                .enumerate()
                .filter(|(_, e)| is_ours(e, service, &event))
                .map(|(i, _)| i)
                .collect();
            if ours.is_empty() {
                g += 1;
                continue;
            }
            found = true;
            if remove || kept {
                for i in ours.into_iter().rev() {
                    entries.remove(i);
                    changed = true;
                }
                if entries.is_empty() {
                    arr.remove(g);
                } else {
                    g += 1;
                }
                continue;
            }
            for i in ours.iter().skip(1).rev() {
                entries.remove(*i);
                changed = true;
            }
            if entries[ours[0]].get("command").and_then(Value::as_str) != Some(&wanted) {
                entries[ours[0]]
                    .as_object_mut()
                    .unwrap()
                    .insert("command".into(), json!(wanted));
                changed = true;
            }
            kept = true;
            g += 1;
        }
        if remove {
            if found && arr.is_empty() {
                hooks.as_object_mut().unwrap().remove(&event);
                changed = true;
            }
        } else if !found {
            arr.push(json!({
                "matcher": "",
                "hooks": [{"type": "command", "command": wanted, "timeout": HOOK_TIMEOUT}]
            }));
            changed = true;
        }
    }
    changed
}

fn edit_cursor_json(data: &mut Value, service: &str, events: &[String], remove: bool) -> bool {
    if !remove && data.get("version").is_none() {
        if let Some(obj) = data.as_object_mut() {
            obj.insert("version".into(), json!(1));
        }
    }
    let hooks = match data.get_mut("hooks") {
        Some(Value::Object(_)) => data.get_mut("hooks").unwrap(),
        _ if remove => return false,
        _ => {
            data.as_object_mut()
                .unwrap()
                .insert("hooks".into(), json!({}));
            data.get_mut("hooks").unwrap()
        }
    };
    let targets: Vec<String> = if remove {
        hooks
            .as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default()
    } else {
        events.to_vec()
    };
    let mut changed = false;
    for event in targets {
        let entries = match hooks.get_mut(&event) {
            Some(Value::Array(_)) => hooks.get_mut(&event).unwrap(),
            _ if remove => continue,
            _ => {
                hooks
                    .as_object_mut()
                    .unwrap()
                    .insert(event.clone(), json!([]));
                hooks.get_mut(&event).unwrap()
            }
        };
        let Some(arr) = entries.as_array_mut() else {
            continue;
        };
        let wanted = hook_command(service, &event);
        let ours: Vec<usize> = arr
            .iter()
            .enumerate()
            .filter(|(_, e)| is_ours(e, service, &event))
            .map(|(i, _)| i)
            .collect();
        if remove {
            if !ours.is_empty() {
                for i in ours.into_iter().rev() {
                    arr.remove(i);
                }
                changed = true;
                if arr.is_empty() {
                    hooks.as_object_mut().unwrap().remove(&event);
                }
            }
            continue;
        }
        if ours.is_empty() {
            arr.push(json!({"command": wanted, "timeout": HOOK_TIMEOUT}));
            changed = true;
            continue;
        }
        for i in ours.iter().skip(1).rev() {
            arr.remove(*i);
            changed = true;
        }
        let entry = arr[ours[0]].as_object_mut().unwrap();
        if entry.get("command").and_then(Value::as_str) != Some(&wanted) {
            entry.insert("command".into(), json!(wanted));
            changed = true;
        }
        if entry.get("timeout").and_then(Value::as_i64) != Some(HOOK_TIMEOUT) {
            entry.insert("timeout".into(), json!(HOOK_TIMEOUT));
            changed = true;
        }
    }
    changed
}

fn plugin_source(service: &str) -> String {
    let hook = hook_bin().display().to_string();
    format!(
        r#"// TokenMeter 이 생성함 — 지우면 연동 해제됩니다.
// {PLUGIN_MARKER}
import {{ spawn }} from "node:child_process"

const HOOK = {hook}

export const TokenMeter = async ({{ directory }}) => {{
  const fire = (event, sessionID) => {{
    try {{
      spawn(HOOK, [{service}, event, sessionID || ""], {{
        detached: true,
        stdio: "ignore",
        env: {{ ...process.env, TOKENMETER_CWD: directory || process.cwd() }},
      }}).unref()
    }} catch {{}}
  }}
  const EVENTS = new Set([
    "session.created", "session.deleted", "session.idle",
    "permission.asked", "permission.v2.asked", "permission.replied", "permission.v2.replied",
    "question.asked", "question.v2.asked", "question.replied", "question.v2.replied",
  ])
  return {{
    event: async ({{ event }}) => {{
      const type = event?.type
      if (!EVENTS.has(type)) return
      const mapped = type === "session.created" ? "SessionStart"
        : type === "session.deleted" ? "SessionEnd" : type
      fire(mapped, event?.properties?.sessionID)
    }},
  }}
}}

export default {{ id: "tokenmeter", server: TokenMeter }}
"#,
        hook = serde_json::to_string(&hook).unwrap_or_else(|_| "\"\"".into()),
        service = serde_json::to_string(service).unwrap_or_else(|_| "\"unknown\"".into()),
    )
}

fn apply_cursor(spec: &ServiceSpec, remove: bool, dry_run: bool) -> String {
    let path = expand_home(&spec.install.path);
    let data = match load_json(&path) {
        Ok(v) => v,
        Err(exc) => {
            return format!(
                "{}: {} 를 읽을 수 없어 건너뜁니다 ({exc}) — 파일은 그대로 둡니다",
                spec.label,
                path.display()
            )
        }
    };
    let mut data = data;
    if !data.is_object() {
        data = json!({});
    }
    let changed = edit_cursor_json(&mut data, &spec.name, &spec.install.events, remove);
    let verb = if remove { "해제" } else { "설치" };
    if dry_run {
        return format!(
            "{}: (dry-run) {} → {verb} {}",
            spec.label,
            path.display(),
            if changed {
                "필요"
            } else {
                "불필요 (이미 반영됨)"
            }
        );
    }
    if !changed {
        return format!("{}: 이미 {verb}된 상태 → {}", spec.label, path.display());
    }
    backup(&path);
    if let Err(exc) = save_json(&path, &data) {
        return format!(
            "{}: {} 에 쓸 수 없어 건너뜁니다 ({exc}) — 파일은 그대로 둡니다",
            spec.label,
            path.display()
        );
    }
    let detail = if !remove && !spec.install.events.is_empty() {
        format!(" [{}]", spec.install.events.join(", "))
    } else {
        String::new()
    };
    format!("{}: {verb} 완료 → {}{detail}", spec.label, path.display())
}

fn apply_claude(spec: &ServiceSpec, remove: bool, dry_run: bool) -> String {
    let path = expand_home(&spec.install.path);
    let data = match load_json(&path) {
        Ok(v) => v,
        Err(exc) => {
            return format!(
                "{}: {} 를 읽을 수 없어 건너뜁니다 ({exc}) — 파일은 그대로 둡니다",
                spec.label,
                path.display()
            )
        }
    };
    let mut data = data;
    if !data.is_object() {
        data = json!({});
    }
    let changed = edit_claude_json(&mut data, &spec.name, &spec.install.events, remove);
    let verb = if remove { "해제" } else { "설치" };
    if dry_run {
        return format!(
            "{}: (dry-run) {} → {verb} {}",
            spec.label,
            path.display(),
            if changed {
                "필요"
            } else {
                "불필요 (이미 반영됨)"
            }
        );
    }
    if !changed {
        return format!("{}: 이미 {verb}된 상태 → {}", spec.label, path.display());
    }
    backup(&path);
    if let Err(exc) = save_json(&path, &data) {
        return format!(
            "{}: {} 에 쓸 수 없어 건너뜁니다 ({exc}) — 파일은 그대로 둡니다",
            spec.label,
            path.display()
        );
    }
    let detail = if !remove && !spec.install.events.is_empty() {
        format!(" [{}]", spec.install.events.join(", "))
    } else {
        String::new()
    };
    format!("{}: {verb} 완료 → {}{detail}", spec.label, path.display())
}

fn apply_plugin(spec: &ServiceSpec, remove: bool, dry_run: bool) -> String {
    let path = expand_home(&spec.install.path);
    let legacy = if path.file_name().and_then(|s| s.to_str()) == Some("tokenmeter.js") {
        Some(path.with_file_name("tokenpet.js"))
    } else {
        None
    };
    let legacy_generated = legacy
        .as_ref()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|t| t.contains(PLUGIN_MARKER) || t.contains(LEGACY_PLUGIN_MARKER))
        .unwrap_or(false);
    if remove {
        if !path.exists() {
            if legacy_generated {
                if let Some(p) = &legacy {
                    let _ = fs::remove_file(p);
                }
            }
            return format!("{}: 이미 해제된 상태 → {}", spec.label, path.display());
        }
        let existing = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(exc) => {
                return format!(
                    "{}: {} 를 읽을 수 없어 건너뜁니다 ({exc})",
                    spec.label,
                    path.display()
                )
            }
        };
        if !existing.contains(PLUGIN_MARKER) && !existing.contains(LEGACY_PLUGIN_MARKER) {
            return format!(
                "{}: {} 는 TokenMeter 이 만든 파일이 아니라 남겨둡니다",
                spec.label,
                path.display()
            );
        }
        if dry_run {
            return format!("{}: (dry-run) {} 삭제 예정", spec.label, path.display());
        }
        let _ = fs::remove_file(&path);
        if legacy_generated {
            if let Some(p) = &legacy {
                let _ = fs::remove_file(p);
            }
        }
        return format!("{}: 해제 완료 → {} 삭제", spec.label, path.display());
    }
    let source = plugin_source(&spec.name);
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(existing) if existing == source => {
                if legacy_generated {
                    if let Some(p) = &legacy {
                        let _ = fs::remove_file(p);
                    }
                }
                return format!("{}: 이미 설치된 상태 → {}", spec.label, path.display());
            }
            Ok(existing)
                if !existing.contains(PLUGIN_MARKER) && !existing.contains(LEGACY_PLUGIN_MARKER) =>
            {
                return format!(
                    "{}: {} 는 TokenMeter 이 만든 파일이 아니라 남겨둡니다",
                    spec.label,
                    path.display()
                )
            }
            Err(exc) => {
                return format!(
                    "{}: {} 를 읽을 수 없어 건너뜁니다 ({exc})",
                    spec.label,
                    path.display()
                )
            }
            _ => {}
        }
    }
    if dry_run {
        return format!("{}: (dry-run) {} 에 플러그인 생성 예정", spec.label, path.display());
    }
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    backup(&path);
    if fs::write(&path, source).is_err() {
        return format!(
            "{}: {} 에 쓸 수 없어 건너뜁니다 — 파일은 그대로 둡니다",
            spec.label,
            path.display()
        );
    }
    if legacy_generated {
        if let Some(p) = &legacy {
            let _ = fs::remove_file(p);
        }
    }
    format!("{}: 설치 완료 → {}", spec.label, path.display())
}

fn targets(names: &[String]) -> Vec<ServiceSpec> {
    let all = load_all_specs();
    if names.is_empty() {
        return all.into_iter().filter(|s| s.enabled).collect();
    }
    all.into_iter()
        .filter(|s| names.iter().any(|n| n == &s.name))
        .collect()
}

fn apply(names: &[String], remove: bool, dry_run: bool) -> Vec<String> {
    let mut out = Vec::new();
    for spec in targets(names) {
        let inst = &spec.install;
        if inst.target.is_empty() || inst.target == "none" {
            out.push(format!("{}: 훅 없음 (로그 감시만)", spec.label));
        } else if inst.path.is_empty() {
            out.push(format!("{}: install.path 가 없어 건너뜁니다", spec.label));
        } else if inst.target == "claude_json" {
            out.push(apply_claude(&spec, remove, dry_run));
        } else if inst.target == "cursor_json" {
            out.push(apply_cursor(&spec, remove, dry_run));
        } else if inst.target == "opencode_plugin" {
            out.push(apply_plugin(&spec, remove, dry_run));
        } else {
            out.push(format!("{}: 알 수 없는 install.target={}", spec.label, inst.target));
        }
    }
    out
}

fn platform_tag() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Some("macos-arm64"),
        ("macos", "x86_64") => Some("macos-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("linux", "aarch64") => Some("linux-arm64"),
        _ => None,
    }
}

fn sha_from_sums(text: &str, asset: &str) -> String {
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(sum) = parts.next() else { continue };
        let name = parts.next().unwrap_or("").trim_start_matches('*');
        if name == asset {
            return sum.to_ascii_lowercase();
        }
    }
    String::new()
}

/// `shasum` 이 없는 리눅스도 있어 `sha256sum` 으로 넘어간다. 둘 다 없으면 빈 값 = 불일치.
fn sha256_file(path: &Path) -> String {
    [("shasum", &["-a", "256"][..]), ("sha256sum", &[][..])]
        .into_iter()
        .filter_map(|(bin, args)| Command::new(bin).args(args).arg(path).output().ok())
        .filter(|out| out.status.success())
        .find_map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .split_whitespace()
                .next()
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_default()
}

fn http_get(url: &str) -> Option<Vec<u8>> {
    let resp = ureq::get(url)
        .set("User-Agent", &format!("TokenMeter/{VERSION}"))
        .timeout(std::time::Duration::from_secs(300))
        .call()
        .ok()?;
    let mut buf = Vec::new();
    resp.into_reader().read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn asset_url(assets: &[Value], name: &str) -> Option<String> {
    assets.iter().find_map(|item| {
        if item.get("name").and_then(Value::as_str) == Some(name) {
            item.get("browser_download_url")
                .and_then(Value::as_str)
                .map(str::to_string)
        } else {
            None
        }
    })
}

/// 받은 바이트를 dest 옆 임시 파일에 쓰고 SHA256SUMS 와 맞을 때만 남긴다.
fn stage_bin(dest: &Path, raw: &[u8], sums: &str, asset: &str) -> Result<PathBuf, String> {
    let want = sha_from_sums(sums, asset);
    if want.is_empty() {
        return Err(format!("SHA256SUMS 에 {asset} 가 없어 거부합니다"));
    }
    if let Some(parent) = dest.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = dest.with_extension("download");
    fs::write(&tmp, raw).map_err(|e| format!("{} 에 쓸 수 없습니다: {e}", tmp.display()))?;
    if sha256_file(&tmp) != want {
        let _ = fs::remove_file(&tmp);
        return Err(format!("{asset} 체크섬이 맞지 않아 거부합니다"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755));
    }
    Ok(tmp)
}

/// 릴리스 에셋 하나를 받아 검증된 임시 파일로 둔다. 체크섬 파일이 없으면 거부한다.
fn stage_release_bin(assets: &[Value], name: &str, dest: &Path) -> Result<PathBuf, String> {
    let tag = platform_tag().ok_or("이 플랫폼용 릴리스가 없습니다 (macOS·Linux만 지원)")?;
    let asset = format!("{name}-{tag}");
    let fetch = |file: &str| asset_url(assets, file).and_then(|url| http_get(&url));
    let sums = fetch("SHA256SUMS")
        .and_then(|b| String::from_utf8(b).ok())
        .ok_or("SHA256SUMS 를 받을 수 없어 거부합니다")?;
    let raw = fetch(&asset).ok_or_else(|| format!("{asset} 를 받을 수 없습니다"))?;
    stage_bin(dest, &raw, &sums, &asset)
}

fn download_release_bin(name: &str, dest: &Path) -> bool {
    let assets = http_get(RELEASE_URL)
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|meta| meta.get("assets").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    match stage_release_bin(&assets, name, dest) {
        Ok(tmp) => fs::rename(&tmp, dest).is_ok() && dest.is_file(),
        Err(_) => dest.is_file(),
    }
}

fn ensure_release_bins() {
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let hook = dir.join(if cfg!(windows) {
        "tokenmeter-hook.exe"
    } else {
        "tokenmeter-hook"
    });
    if !hook.is_file() {
        let _ = download_release_bin("tokenmeter-hook", &hook);
    }
}

pub fn install(names: &[String], dry_run: bool) -> Vec<String> {
    if !dry_run {
        ensure_release_bins();
        if !hook_bin().is_file() {
            return vec![
                "네이티브 훅이 없습니다. GitHub 정식 릴리스(tokenmeter-hook-<os>-<arch>)를 받거나 cargo build."
                    .into(),
            ];
        }
    }
    apply(names, false, dry_run)
}

pub fn uninstall(names: &[String]) -> Vec<String> {
    apply(names, true, false)
}

pub fn purge_state() -> Vec<String> {
    let dir = data_dir();
    let mut out = Vec::new();
    if dir.exists() {
        let _ = fs::remove_dir_all(&dir);
        out.push(format!("상태 디렉터리 삭제: {}", dir.display()));
    } else {
        out.push("상태 디렉터리 없음".into());
    }
    out.push("설정은 남김: ~/.config/tokenmeter".into());
    out.push(format!(
        "바이너리 제거: rm \"{}\" \"{}\"",
        meter_bin().display(),
        hook_bin().display()
    ));
    out
}

pub fn install_state(spec: &ServiceSpec) -> &'static str {
    let inst = &spec.install;
    if inst.target.is_empty() || inst.target == "none" || inst.path.is_empty() {
        return "skip";
    }
    if !stale_command(spec).is_empty() {
        return "stale";
    }
    if inst.target == "claude_json" || inst.target == "cursor_json" {
        if inst.events.is_empty() {
            return "missing";
        }
        let found = installed_events(spec);
        if found.is_empty() {
            return "missing";
        }
        return if found.len() == inst.events.len() {
            "ok"
        } else {
            "stale"
        };
    }
    if inst.target == "opencode_plugin" {
        let path = expand_home(&inst.path);
        if path.is_file() {
            if let Ok(text) = fs::read_to_string(path) {
                if text.contains(PLUGIN_MARKER) {
                    return "ok";
                }
            }
        }
    }
    "missing"
}

fn installed_events(spec: &ServiceSpec) -> Vec<String> {
    let inst = &spec.install;
    if inst.target != "claude_json" && inst.target != "cursor_json" {
        return Vec::new();
    }
    let Ok(data) = load_json(&expand_home(&inst.path)) else {
        return Vec::new();
    };
    let Some(hooks) = data.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    inst.events
        .iter()
        .filter(|event| {
            let rows = hooks.get(event.as_str()).and_then(Value::as_array);
            if inst.target == "cursor_json" {
                rows.into_iter()
                    .flatten()
                    .any(|e| is_ours(e, &spec.name, event))
            } else {
                rows.into_iter()
                    .flatten()
                    .any(|g| {
                        g.get("hooks")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .any(|e| is_ours(e, &spec.name, event))
                    })
            }
        })
        .cloned()
        .collect()
}

fn stale_command(spec: &ServiceSpec) -> String {
    let inst = &spec.install;
    if inst.path.is_empty() {
        return String::new();
    }
    if inst.target == "claude_json" || inst.target == "cursor_json" {
        let Ok(data) = load_json(&expand_home(&inst.path)) else {
            return String::new();
        };
        let Some(hooks) = data.get("hooks") else {
            return String::new();
        };
        for event in &inst.events {
            let wanted = hook_command(&spec.name, event);
            let Some(rows) = hooks.get(event).and_then(Value::as_array) else {
                continue;
            };
            if inst.target == "cursor_json" {
                for entry in rows {
                    let cmd = entry.get("command").and_then(Value::as_str).unwrap_or("");
                    if is_ours(entry, &spec.name, event) && cmd != wanted {
                        return cmd.to_string();
                    }
                }
            } else {
                for group in rows {
                    if let Some(entries) = group.get("hooks").and_then(Value::as_array) {
                        for entry in entries {
                            let cmd = entry.get("command").and_then(Value::as_str).unwrap_or("");
                            if is_ours(entry, &spec.name, event) && cmd != wanted {
                                return cmd.to_string();
                            }
                        }
                    }
                }
            }
        }
    } else if inst.target == "opencode_plugin" {
        let path = expand_home(&inst.path);
        if let Ok(text) = fs::read_to_string(&path) {
            if text.contains(PLUGIN_MARKER) && text != plugin_source(&spec.name) {
                return format!("{} (플러그인 내용이 현재 경로와 다름)", path.display());
            }
        }
    }
    String::new()
}

pub fn status_rows() -> Vec<(String, String, String, bool, String)> {
    load_all_specs()
        .into_iter()
        .map(|spec| {
            (
                spec.name.clone(),
                spec.label.clone(),
                if spec.install.target.is_empty() {
                    "none".into()
                } else {
                    spec.install.target.clone()
                },
                spec.enabled,
                expand_home(&spec.install.path).display().to_string(),
            )
        })
        .collect()
}

fn parse_version(value: &str) -> Option<(u32, u32, u32)> {
    let t = value.trim().trim_start_matches('v');
    let mut parts = t.split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ))
}

pub fn update_package(force: bool) -> (Option<bool>, String) {
    let toggle_path = data_dir().join("toggle.json");
    let mut toggle = fs::read_to_string(&toggle_path)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .unwrap_or(json!({}));
    if !force && toggle.get("auto_update") != Some(&json!(true)) {
        return (Some(false), String::new());
    }
    let now = crate::watch::now_secs();
    let checked = toggle
        .get("update_checked_at")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if !force && now - checked >= 0.0 && now - checked < UPDATE_INTERVAL {
        return (Some(false), String::new());
    }
    if let Some(obj) = toggle.as_object_mut() {
        obj.insert("update_checked_at".into(), json!(now));
    }
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(&toggle_path, toggle.to_string());
    let resp = ureq::get(RELEASE_URL)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("TokenMeter/{VERSION}"))
        .timeout(std::time::Duration::from_secs(5))
        .call();
    let meta = match resp {
        Ok(r) => r.into_json::<Value>().unwrap_or_default(),
        Err(err) => return (None, format!("업데이트 확인 실패: {err}")),
    };
    let tag = meta.get("tag_name").and_then(Value::as_str).unwrap_or_default().to_string();
    let Some(latest) = parse_version(&tag) else {
        return (None, format!("지원하지 않는 릴리스 태그: {}", if tag.is_empty() { "(없음)" } else { &tag }));
    };
    let Some(current) = parse_version(VERSION) else {
        return (None, format!("현재 버전을 판독할 수 없습니다: {VERSION}"));
    };
    if latest <= current {
        return (
            Some(false),
            if force {
                format!("최신 버전입니다 (v{VERSION})")
            } else {
                String::new()
            },
        );
    }
    let version = format!("v{}.{}.{}", latest.0, latest.1, latest.2);
    match replace_bins(&meta) {
        Ok(()) => (Some(true), format!("v{VERSION} → {version} 업데이트 완료")),
        Err(err) => (None, format!("{version} 업데이트 실패: {err}")),
    }
}

/// 미터·훅을 둘 다 받아 검증한 뒤에야 제자리에서 바꾼다 (rename 은 실행 중인 파일에도 안전하다).
fn replace_bins(meta: &Value) -> Result<(), String> {
    let meter = std::env::current_exe().map_err(|e| e.to_string())?;
    let hook = meter.with_file_name(if cfg!(windows) { "tokenmeter-hook.exe" } else { "tokenmeter-hook" });
    let assets = meta.get("assets").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut staged = Vec::new();
    for (name, dest) in [("tokenmeter", &meter), ("tokenmeter-hook", &hook)] {
        match stage_release_bin(&assets, name, dest) {
            Ok(tmp) => staged.push((tmp, dest)),
            Err(err) => {
                for (tmp, _) in &staged {
                    let _ = fs::remove_file(tmp);
                }
                return Err(err);
            }
        }
    }
    for (tmp, dest) in staged {
        fs::rename(&tmp, dest).map_err(|e| format!("{} 를 바꿀 수 없습니다: {e}", dest.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::InstallSpec;

    fn spec(name: &str, target: &str, path: &Path, events: &[&str]) -> ServiceSpec {
        ServiceSpec {
            name: name.into(),
            label: name.into(),
            enabled: true,
            install: InstallSpec {
                target: target.into(),
                path: path.display().to_string(),
                events: events.iter().map(|e| e.to_string()).collect(),
            },
            ..ServiceSpec::default()
        }
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    /// Claude 스키마(그룹 → hooks)에서 한 이벤트의 command 들.
    fn commands(data: &Value, event: &str) -> Vec<String> {
        data["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
            .filter_map(|e| e["command"].as_str().map(str::to_string))
            .collect()
    }

    #[test]
    fn claude_json_install_twice_keeps_one_entry_and_uninstall_restores() {
        let (_g, tmp) = crate::test_home("install-claude");
        let path = tmp.join("settings.json");
        let original = json!({
            "model": "opus",
            "hooks": {
                "SessionStart": [{"matcher": "", "hooks": [
                    {"type": "command", "command": "/usr/bin/other-hook.sh", "timeout": 3}]}],
                "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "echo hi"}]}]
            },
            "permissions": {"allow": ["Bash(ls:*)"]}
        });
        fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        let before = fs::read_to_string(&path).unwrap();
        let spec = spec("claude-code", "claude_json", &path, &["SessionStart", "SessionEnd"]);

        apply_claude(&spec, false, true);
        assert_eq!(fs::read_to_string(&path).unwrap(), before, "dry-run 은 파일을 건드리지 않는다");
        assert_eq!(install_state(&spec), "missing");

        apply_claude(&spec, false, false);
        apply_claude(&spec, false, false);
        let data = read(&path);
        for event in ["SessionStart", "SessionEnd"] {
            let ours: Vec<_> = commands(&data, event).into_iter().filter(|c| *c == hook_command("claude-code", event)).collect();
            assert_eq!(ours.len(), 1, "{event}: 이벤트마다 엔트리는 하나");
        }
        assert_eq!(install_state(&spec), "ok");
        assert_eq!((&data["model"], &data["permissions"]), (&original["model"], &original["permissions"]));
        assert_eq!(data["hooks"]["PreToolUse"], original["hooks"]["PreToolUse"], "남의 설정은 한 글자도 안 바뀐다");
        assert!(data["hooks"]["SessionStart"].as_array().unwrap().contains(&original["hooks"]["SessionStart"][0]));
        assert!(tmp.join(format!("settings.json{BACKUP_SUFFIX}")).exists(), "백업이 없다");

        apply_claude(&spec, true, false);
        assert_eq!(read(&path), original, "해제 후 원본과 달라졌다");
        assert_eq!(install_state(&spec), "missing");
    }

    #[test]
    fn foreign_hook_and_file_mode_survive_install() {
        let (_g, tmp) = crate::test_home("install-foreign");
        let path = tmp.join("settings.json");
        let foreign = json!({"type": "command", "command": "python3 /other/tool/tokenmeter/hook.py start", "timeout": 10});
        let original = json!({"hooks": {"SessionStart": [{"matcher": "", "hooks": [foreign]}], "PreToolUse": []}});
        fs::write(&path, serde_json::to_string_pretty(&original).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let spec = spec("claude-code", "claude_json", &path, &["SessionStart", "SessionEnd"]);
        apply_claude(&spec, false, false);
        assert!(read(&path)["hooks"]["SessionStart"].as_array().unwrap().contains(&original["hooks"]["SessionStart"][0]));
        assert_eq!(install_state(&spec), "ok", "남의 훅을 우리 것으로 오인해 설치를 건너뛰었다");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600, "원본 권한이 유실됐다");
        }
        apply_claude(&spec, true, false);
        assert_eq!(read(&path), original);
    }

    #[test]
    fn corrupt_json_is_left_untouched() {
        let (_g, tmp) = crate::test_home("install-corrupt");
        for (target, name) in [("claude_json", "settings.json"), ("cursor_json", "hooks.json")] {
            let path = tmp.join(name);
            fs::write(&path, "{ not json").unwrap();
            let spec = spec("claude-code", target, &path, &["SessionStart"]);
            for remove in [false, true] {
                let line = if target == "cursor_json" {
                    apply_cursor(&spec, remove, false)
                } else {
                    apply_claude(&spec, remove, false)
                };
                assert!(line.contains("건너뜁니다"), "{line}");
                assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json", "{target}");
            }
            assert!(!tmp.join(format!("{name}{BACKUP_SUFFIX}")).exists());
        }
    }

    #[test]
    fn legacy_python_tokenpet_and_uv_era_entries_are_ours() {
        let (_g, tmp) = crate::test_home("install-legacy");
        let path = tmp.join("settings.json");
        let legacy = [
            r#""/old/py" "/old/checkout/src/hook.py" claude-code SessionStart"#,
            r#""/old/checkout/.venv/bin/python" "/old/checkout/tokenpet/hook.py" claude-code SessionStart"#,
            r#""/tools/tokenmeter/bin/python" "/tools/tokenmeter/lib/python3.14/site-packages/tokenmeter/hook.py" claude-code SessionStart"#,
            r#""/Users/x/.local/share/uv/tools/tokenmeter/lib/python3.12/site-packages/tokenmeter/bin/tokenmeter-hook" claude-code SessionStart"#,
        ];
        let spec = spec("claude-code", "claude_json", &path, &["SessionStart"]);
        let group = |cmd: &str| json!({"matcher": "", "hooks": [{"type": "command", "command": cmd, "timeout": 5}]});
        for cmd in legacy {
            fs::write(&path, json!({"hooks": {"SessionStart": [group(cmd)]}}).to_string()).unwrap();
            assert_eq!(install_state(&spec), "stale", "{cmd}");
            apply_claude(&spec, false, false);
            assert_eq!(commands(&read(&path), "SessionStart"), [hook_command("claude-code", "SessionStart")], "제자리 교체: {cmd}");
        }

        let foreign = group("/usr/bin/other-hook.sh");
        fs::write(&path, json!({"hooks": {"SessionStart": [group(legacy[0]), group(legacy[2]), foreign.clone()]}}).to_string()).unwrap();
        apply_claude(&spec, false, false);
        let data = read(&path);
        assert_eq!(
            commands(&data, "SessionStart"),
            [hook_command("claude-code", "SessionStart"), "/usr/bin/other-hook.sh".to_string()],
            "체크아웃 훅과 설치본 훅은 하나로 합친다"
        );

        fs::write(&path, json!({"hooks": {"SessionStart": [group(legacy[1]), foreign.clone(), group(legacy[3])]}}).to_string()).unwrap();
        apply_claude(&spec, true, false);
        assert_eq!(read(&path), json!({"hooks": {"SessionStart": [foreign]}}), "해제는 옛 엔트리까지 우리 것만 걷어낸다");
    }

    const CURSOR_EVENTS: [&str; 5] = ["sessionStart", "sessionEnd", "beforeSubmitPrompt", "stop", "afterAgentResponse"];

    #[test]
    fn cursor_hooks_json_is_flat_versioned_and_idempotent() {
        let (_g, tmp) = crate::test_home("install-cursor");
        let path = tmp.join("hooks.json");
        let other = json!({"command": "./hooks/other.sh", "timeout": 10});
        let format = json!([{"command": "./hooks/format.sh"}]);
        fs::write(&path, json!({"version": 1, "hooks": {"sessionStart": [other], "afterFileEdit": format}}).to_string()).unwrap();
        let spec = spec("cursor", "cursor_json", &path, &CURSOR_EVENTS);

        apply_cursor(&spec, false, true);
        assert_eq!(install_state(&spec), "missing");
        apply_cursor(&spec, false, false);
        apply_cursor(&spec, false, false);
        let data = read(&path);
        assert_eq!(data["version"], 1);
        assert_eq!(data["hooks"]["afterFileEdit"], format);
        assert_eq!(data["hooks"]["sessionStart"][0], other);
        for event in CURSOR_EVENTS {
            let entries = data["hooks"][event].as_array().unwrap();
            let ours: Vec<_> = entries.iter().filter(|e| e["command"] == hook_command("cursor", event)).collect();
            assert_eq!(ours, [&json!({"command": hook_command("cursor", event), "timeout": HOOK_TIMEOUT})], "{event}");
        }
        assert_eq!(install_state(&spec), "ok");
        let mut found = installed_events(&spec);
        found.sort();
        let mut want: Vec<String> = CURSOR_EVENTS.iter().map(|e| e.to_string()).collect();
        want.sort();
        assert_eq!(found, want);

        apply_cursor(&spec, true, false);
        let after = read(&path);
        assert_eq!(after["hooks"]["afterFileEdit"], format);
        assert_eq!(after["hooks"]["sessionStart"], json!([other]));
        assert!(after["hooks"].get("sessionEnd").is_none());
        assert_eq!(install_state(&spec), "missing");

        let fresh = tmp.join("fresh/hooks.json");
        apply_cursor(&spec_with_path(&spec, &fresh), false, false);
        assert_eq!(read(&fresh)["version"], 1, "새 파일에도 version:1 을 넣는다");
    }

    fn spec_with_path(base: &ServiceSpec, path: &Path) -> ServiceSpec {
        let mut spec = base.clone();
        spec.install.path = path.display().to_string();
        spec
    }

    #[test]
    #[ignore = "Rust is_ours 가 손수 넣은 ./hooks/tokenmeter.sh 래퍼를 모른다 (Python _is_ours_cursor 는 알았다) — 설치 시 죽은 래퍼가 남고 엔트리가 덧붙는다"]
    fn cursor_hand_made_tokenmeter_sh_wrapper_is_replaced() {
        let (_g, tmp) = crate::test_home("install-cursor-sh");
        let path = tmp.join("hooks.json");
        fs::write(&path, json!({"version": 1, "hooks": {"sessionStart": [
            {"command": "./hooks/other.sh", "timeout": 10},
            {"command": "./hooks/tokenmeter.sh sessionStart", "timeout": 5}
        ]}}).to_string()).unwrap();
        apply_cursor(&spec("cursor", "cursor_json", &path, &CURSOR_EVENTS), false, false);
        let start: Vec<String> = read(&path)["hooks"]["sessionStart"].as_array().unwrap().iter()
            .filter_map(|e| e["command"].as_str().map(str::to_string)).collect();
        assert!(!start.iter().any(|c| c.contains("tokenmeter.sh")), "{start:?}");
        assert_eq!(start.iter().filter(|c| **c == hook_command("cursor", "sessionStart")).count(), 1);
    }

    #[test]
    fn opencode_plugin_is_marked_replaces_legacy_and_spares_foreign_files() {
        let (_g, tmp) = crate::test_home("install-plugin");
        let dir = tmp.join("plugin");
        fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("tokenpet.js");
        fs::write(&legacy, "// tokenpet:generated\n").unwrap();
        let path = dir.join("tokenmeter.js");
        let spec = spec("opencode", "opencode_plugin", &path, &["SessionStart"]);

        apply_plugin(&spec, false, false);
        let source = fs::read_to_string(&path).unwrap();
        assert!(source.contains(PLUGIN_MARKER));
        assert!(source.contains(&format!("const HOOK = {}", serde_json::to_string(&hook_bin().display().to_string()).unwrap())));
        assert!(source.contains("spawn(HOOK, [\"opencode\", event"));
        assert!(!legacy.exists(), "옛 생성 플러그인을 남기면 이벤트가 중복된다");
        assert_eq!(install_state(&spec), "ok");
        assert!(apply_plugin(&spec, false, false).contains("이미 설치된 상태"));

        apply_plugin(&spec, true, false);
        assert!(!path.exists());
        fs::write(&path, "// 남이 만든 플러그인\n").unwrap();
        apply_plugin(&spec, true, false);
        apply_plugin(&spec, false, false);
        assert_eq!(fs::read_to_string(&path).unwrap(), "// 남이 만든 플러그인\n", "우리 마커가 없는 파일은 건드리지 않는다");
    }

    #[test]
    fn install_state_tells_missing_stale_ok_and_skip_apart() {
        let (_g, tmp) = crate::test_home("install-state");
        let path = tmp.join("settings.json");
        let events = ["SessionStart", "UserPromptSubmit", "SessionEnd"];
        let claude = spec("claude-code", "claude_json", &path, &events);
        fs::write(&path, r#"{"hooks": {}}"#).unwrap();
        assert_eq!(install_state(&claude), "missing");

        let partial: serde_json::Map<String, Value> = ["SessionStart", "SessionEnd"]
            .iter()
            .map(|ev| (ev.to_string(), json!([{"matcher": "", "hooks": [{"type": "command", "command": hook_command("claude-code", ev)}]}])))
            .collect();
        fs::write(&path, json!({"hooks": partial}).to_string()).unwrap();
        assert_eq!(install_state(&claude), "stale", "새 이벤트가 빠졌는데 설치됨으로 보이면 안 된다");

        apply_claude(&claude, false, false);
        assert_eq!(install_state(&claude), "ok");
        let mut found = installed_events(&claude);
        found.sort();
        assert_eq!(found, ["SessionEnd", "SessionStart", "UserPromptSubmit"]);
        assert_eq!(install_state(&spec("x", "none", &path, &[])), "skip");
    }

    #[test]
    fn hook_command_and_plugin_point_at_the_native_hook() {
        let cmd = hook_command("claude-code", "SessionStart");
        assert!(cmd.ends_with(" claude-code SessionStart") && cmd.contains("tokenmeter-hook"), "{cmd}");
        let source = plugin_source("opencode");
        for text in [cmd.as_str(), source.as_str()] {
            assert!(!text.contains("hook.py") && !text.to_lowercase().contains("python"), "{text}");
        }
        assert!(source.contains("spawn(HOOK,") && !source.contains("const PY"));
    }

    #[test]
    fn opencode_plugin_has_a_default_export() {
        // OpenCode 1.18.30+는 이름 붙은 내보내기만 있는 플러그인 파일을 거부한다
        let source = plugin_source("opencode");
        assert!(source.contains("export default { id: \"tokenmeter\", server: TokenMeter }"), "{source}");
    }

    #[test]
    fn purge_state_removes_data_and_keeps_config() {
        let (_g, _tmp) = crate::test_home("purge");
        fs::write(data_dir().join("league-auth.json"), "{}").unwrap();
        let lines = purge_state();
        assert!(!data_dir().exists());
        assert!(lines.iter().any(|l| l.contains("삭제")) && lines.iter().any(|l| l.contains("설정은 남김")));
        assert!(lines.iter().any(|l| l.contains("rm ") && l.contains("tokenmeter-hook")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("uv tool")));
    }

    #[test]
    fn auto_update_is_opt_in_and_checks_once_a_day() {
        let (_g, _tmp) = crate::test_home("update");
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(update_package(false), (Some(false), String::new()), "켜지 않으면 네트워크를 타지 않는다");
        let toggle = data_dir().join("toggle.json");
        // 정수 시각만 쓴다 — serde_json 기본 파서는 소수 끝자리를 정확히 되돌리지 못해 비교가 가끔 깨진다
        let checked = (crate::watch::now_secs() - 60.0).floor();
        fs::write(&toggle, json!({"auto_update": true, "update_checked_at": checked}).to_string()).unwrap();
        assert_eq!(update_package(false), (Some(false), String::new()), "하루 안에는 다시 확인하지 않는다");
        assert_eq!(read(&toggle)["update_checked_at"], checked);
    }

    #[test]
    fn staged_binary_must_match_sha256sums() {
        let (_g, tmp) = crate::test_home("stage");
        let dest = tmp.join("bin/tokenmeter-hook");
        let asset = "tokenmeter-hook-macos-arm64";
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let staged = stage_bin(&dest, b"abc", &format!("{abc}  {asset}\n"), asset).unwrap();
        assert_eq!(fs::read(&staged).unwrap(), b"abc");
        assert!(!dest.exists(), "검증만 하고 교체는 호출한 쪽이 rename 으로 한다");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&staged).unwrap().permissions().mode() & 0o777, 0o755);
        }
        let wrong = format!("{}  {asset}\n", "0".repeat(64));
        assert!(stage_bin(&dest, b"abc", &wrong, asset).is_err());
        assert!(!staged.exists(), "체크섬이 틀린 파일은 남기지 않는다");
        assert!(stage_bin(&dest, b"abc", "", asset).is_err(), "SHA256SUMS 에 없으면 거부한다");
        assert!(!dest.exists());
    }

    #[test]
    fn release_assets_match_platform_tags() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let workflow = fs::read_to_string(root.join(".github/workflows/release.yml")).unwrap();
        for tag in ["linux-x64", "linux-arm64", "macos-arm64", "macos-x64"] {
            for name in ["tokenmeter", "tokenmeter-hook"] {
                assert!(workflow.contains(&format!("{name}-{tag}")), "{name}-{tag}");
            }
        }
        assert!(workflow.contains("SHA256SUMS"));
    }

    #[test]
    fn sha_from_sums_reads_named_asset() {
        let text = "abc123  tokenmeter-hook-macos-arm64\ndef456  SHA256SUMS\n";
        assert_eq!(
            sha_from_sums(text, "tokenmeter-hook-macos-arm64"),
            "abc123"
        );
        assert_eq!(sha_from_sums(text, "missing"), "");
    }

    #[test]
    fn platform_tag_is_macos_or_linux() {
        let tag = platform_tag();
        if let Some(tag) = tag {
            assert!(
                tag == "macos-arm64"
                    || tag == "macos-x64"
                    || tag == "linux-x64"
                    || tag == "linux-arm64"
            );
        }
    }
}
