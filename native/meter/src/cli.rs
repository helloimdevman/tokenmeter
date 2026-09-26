//! 네이티브 CLI. 제품의 유일한 사용자 진입점이다.

use crate::adapter::{check_adapter, init_adapter};
use crate::attention::session_views;
use crate::board;
use crate::engine::Meter;
use crate::install;
use crate::league;
use crate::live_rate::LiveRate;
use crate::overlay::snapshot_from;
use crate::pricing;
use crate::quota;
use crate::share;
use crate::watch::checkpoint::Store;
use crate::watch::roots::{self, Vars};
use crate::watch::time::local_date;
use crate::watch::{
    expand_home, is_static_builtin_root, load_all_specs, load_report, now_secs, root_templates, ServiceReader, ServiceSpec,
    TokenDelta,
};
use crate::VERSION;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tokenmeter_hook::{data_dir, is_live_daemon_command};

struct Args {
    cmd: String,
    rest: Vec<String>,
    flags: HashMap<String, Vec<String>>,
    bools: HashMap<String, bool>,
}

fn parse(argv: &[String]) -> Args {
    let mut rest = Vec::new();
    let mut flags: HashMap<String, Vec<String>> = HashMap::new();
    let mut bools = HashMap::new();
    let mut i = 0;
    let cmd = match argv.first().map(String::as_str) {
        None => "daemon".into(),
        Some("-h") | Some("--help") => "--help".into(),
        Some("-V") | Some("--version") => "--version".into(),
        Some(s) if s.starts_with('-') => "daemon".into(),
        Some(s) => {
            i = 1;
            s.to_string()
        }
    };
    while i < argv.len() {
        let a = &argv[i];
        if let Some(name) = a.strip_prefix("--") {
            if matches!(
                name,
                "json" | "jsonl" | "cached" | "sync" | "dry-run" | "yes" | "no-overlay" | "no-window"
                    | "purge"
            ) {
                bools.insert(name.into(), true);
            } else if i + 1 < argv.len() && !argv[i + 1].starts_with('-') {
                flags.entry(name.into()).or_default().push(argv[i + 1].clone());
                i += 1;
            } else {
                bools.insert(name.into(), true);
            }
        } else {
            rest.push(a.clone());
        }
        i += 1;
    }
    Args {
        cmd,
        rest,
        flags,
        bools,
    }
}

fn flag<'a>(args: &'a Args, name: &str) -> Option<&'a str> {
    args.flags.get(name).and_then(|v| v.first()).map(String::as_str)
}

fn flags(args: &Args, name: &str) -> Vec<String> {
    args.flags.get(name).cloned().unwrap_or_default()
}

fn on(args: &Args, name: &str) -> bool {
    args.bools.get(name).copied().unwrap_or(false)
}

fn pad(text: &str, width: usize, right: bool) -> String {
    let w = text.chars().map(|c| if (c as u32) > 0x2E80 { 2 } else { 1 }).sum::<usize>();
    let gap = " ".repeat(width.saturating_sub(w));
    if right {
        format!("{gap}{text}")
    } else {
        format!("{text}{gap}")
    }
}

fn table(headers: &[&str], rows: &[Vec<String>], right: &[usize]) {
    if headers.is_empty() {
        return;
    }
    let mut widths: Vec<usize> = headers
        .iter()
        .map(|h| h.chars().map(|c| if (c as u32) > 0x2E80 { 2 } else { 1 }).sum())
        .collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(
                    cell.chars()
                        .map(|c| if (c as u32) > 0x2E80 { 2 } else { 1 })
                        .sum(),
                );
            }
        }
    }
    let line = |cells: &[String]| {
        format!(
            "  {}",
            cells
                .iter()
                .enumerate()
                .map(|(i, c)| pad(c, widths[i], right.contains(&i)))
                .collect::<Vec<_>>()
                .join("  ")
        )
    };
    println!(
        "{}",
        line(&headers.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    );
    println!(
        "  {}",
        widths
            .iter()
            .map(|w| "─".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!("{}", line(row));
    }
}

fn num(v: i64) -> String {
    let s = v.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    let mut out: String = out.chars().rev().collect();
    if v < 0 {
        out.insert(0, '-');
    }
    out
}

fn usd(v: f64) -> String {
    format!("${v:.4}")
}

fn tokens_of(node: &Value) -> i64 {
    ["input_tokens", "cache_read", "cache_write", "output_tokens"]
        .into_iter()
        .map(|k| node.get(k).and_then(Value::as_i64).unwrap_or(0))
        .sum()
}

fn fnum(v: &Value) -> f64 {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .unwrap_or(0.0)
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
    let out = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output();
    match out {
        Ok(o) => {
            if is_live_daemon_command(&String::from_utf8_lossy(&o.stdout)) {
                pid
            } else {
                0
            }
        }
        Err(_) => pid,
    }
}

fn kill_daemon() -> bool {
    let pid = daemon_pid();
    if pid <= 0 {
        return false;
    }
    std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(crate) fn load_toggle() -> Value {
    let v: Value = fs::read_to_string(data_dir().join("toggle.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(json!({}));
    if v.is_object() {
        v
    } else {
        json!({})
    }
}

pub(crate) fn save_toggle(data: &Value) {
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::write(data_dir().join("toggle.json"), data.to_string());
}

fn spawn_daemon() -> bool {
    if std::env::var("TOKENMETER_NO_DAEMON").ok().as_deref() == Some("1") {
        return false;
    }
    if daemon_pid() != 0 {
        return false;
    }
    let bin = install::meter_bin();
    std::process::Command::new(&bin)
        .arg("daemon")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

fn activation() {
    println!("  첫 측정 시작하기");
    println!("  1. 이미 켜 둔 Claude Code, Codex, OpenCode, Cursor(IDE/CLI)는 완전히 다시 여세요. 재시작 전 세션은 안 잽니다.");
    println!("  2. 미터 창이 바로 뜹니다. 안 보이면 `tokenmeter doctor`.");
    println!("  3. 새 프롬프트를 실행하면 그 세션부터 상태가 보입니다. Cursor는 상태만 잽니다.");
    println!("  4. 금액은 API 환산 추정입니다. 청구서가 아닙니다. Windows는 지원하지 않습니다.");
}

fn services_table() {
    let specs = load_all_specs();
    let mut rows = Vec::new();
    for spec in &specs {
        let roots: Vec<_> = spec
            .roots
            .iter()
            .map(|r| expand_home(r))
            .filter(|p| p.exists())
            .collect();
        let reader = ServiceReader::new(spec.clone());
        let files = reader.files();
        let mtime = files
            .iter()
            .filter_map(|p| p.metadata().and_then(|m| m.modified()).ok())
            .max();
        let when = mtime
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                let ts = d.as_secs();
                format_ts(ts as f64)
            })
            .unwrap_or_else(|| "-".into());
        let state = match install::install_state(spec) {
            "skip" => "불필요",
            "missing" => "미설치",
            "stale" => "갱신 필요",
            _ => "설치됨",
        };
        rows.push(vec![
            spec.name.clone(),
            if spec.enabled { "예" } else { "아니오" }.into(),
            if spec.roots.is_empty() {
                "없음".into()
            } else {
                format!("{}/{}개", roots.len(), spec.roots.len())
            },
            if roots.is_empty() {
                "-".into()
            } else {
                format!("{}개", files.len())
            },
            when,
            if spec.install.target.is_empty() || spec.install.target == "none" {
                "-".into()
            } else {
                spec.install.target.clone()
            },
            state.into(),
        ]);
    }
    table(
        &["서비스", "활성", "로그 경로", "로그 파일", "최근 로그", "훅 대상", "훅 설치"],
        &rows,
        &[3],
    );
    println!();
    println!(
        "  훅 커맨드 : {}",
        install::hook_command("<서비스>", "<이벤트>")
    );
}

fn format_ts(ts: f64) -> String {
    if ts <= 0.0 {
        return "-".into();
    }
    std::process::Command::new("date")
        .args(["-r", &(ts as i64).to_string(), "+%Y-%m-%d %H:%M"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "-".into())
}

fn cmd_install(args: &Args) -> i32 {
    let names = {
        let named = flags(args, "service");
        if named.is_empty() {
            args.rest.clone()
        } else {
            named
        }
    };
    for line in install::install(&names, on(args, "dry-run")) {
        println!("  {line}");
    }
    println!();
    services_table();
    if !on(args, "dry-run") {
        println!();
        if spawn_daemon() {
            println!("  데몬을 띄웠습니다.");
        }
        activation();
        for line in share::ask_once() {
            println!("  {line}");
        }
    }
    0
}

fn cmd_uninstall(args: &Args) -> i32 {
    if on(args, "purge") {
        // 종료 때의 마지막 전송(flush)이 돌지 않게 공유부터 끈다.
        share::set(false);
        let _ = kill_daemon();
    }
    for line in install::uninstall(&flags(args, "service")) {
        println!("  {line}");
    }
    if on(args, "purge") {
        println!();
        // 전부 지우기다. device.json이 지워지면 서버 데이터를 지울 방법이 없으니 먼저 한 번 지워 본다.
        if let Some(token) = crate::server::device_token() {
            match crate::server::delete_account(&token) {
                Ok(()) | Err(crate::server::ApiError::Status(401, _)) => println!("  서버에 보낸 사용 데이터를 지웠습니다."),
                Err(e) => {
                    // device.json 이 서버 데이터를 지울 유일한 열쇠라 상태 폴더를 지우지 않는다.
                    println!("  서버 데이터를 지우지 못했습니다: {}. 서버 데이터는 남았습니다.", api_reason(&e));
                    println!("  다시 지울 수 있게 상태 폴더를 남겼습니다: {}", data_dir().display());
                    let retry = match e {
                        crate::server::ApiError::Off => "settings.league.server 주소를 되돌린 뒤 tokenmeter uninstall --purge",
                        crate::server::ApiError::Upgrade => "tokenmeter update now 뒤 tokenmeter uninstall --purge",
                        _ => "tokenmeter uninstall --purge",
                    };
                    println!("  다시 시도: {retry}");
                    return 1;
                }
            }
        }
        for line in install::purge_state() {
            println!("  {line}");
        }
        return 0;
    }
    println!();
    services_table();
    0
}

fn toggle_measure(on_flag: bool, services: &[String]) -> i32 {
    let specs = load_all_specs();
    let mut data = load_toggle();
    if !services.is_empty() {
        let unknown: Vec<_> = services
            .iter()
            .filter(|s| !specs.iter().any(|sp| sp.name == **s))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            println!("  모르는 서비스: {}", unknown.join(", "));
            return 1;
        }
        let book = data
            .as_object_mut()
            .unwrap()
            .entry("services")
            .or_insert(json!({}));
        for name in services {
            book.as_object_mut()
                .unwrap()
                .insert(name.clone(), json!(on_flag));
        }
        println!(
            "  {} 측정을 {}습니다.",
            services.join(", "),
            if on_flag { "켰습니" } else { "껐습니" }
        );
    } else {
        data.as_object_mut()
            .unwrap()
            .insert("enabled".into(), json!(on_flag));
        println!(
            "  전체 측정을 {}습니다.",
            if on_flag { "켰습니" } else { "껐습니" }
        );
    }
    save_toggle(&data);
    if !on_flag && services.is_empty() && kill_daemon() {
        println!("  실행 중이던 데몬을 종료했습니다.");
    } else if on_flag && spawn_daemon() {
        println!("  데몬을 띄웠습니다.");
    }
    0
}

fn cmd_meter(args: &Args) -> i32 {
    let mut data = load_toggle();
    if args.rest.is_empty() {
        let on_now = data.get("overlay") != Some(&json!(false));
        println!("  미터 자동 표시: {}", if on_now { "켜짐" } else { "꺼짐" });
        return 0;
    }
    if args.rest[0] != "on" && args.rest[0] != "off" {
        println!("  사용법: tokenmeter meter [on|off]");
        return 1;
    }
    let on_now = args.rest[0] == "on";
    data.as_object_mut()
        .unwrap()
        .insert("overlay".into(), json!(on_now));
    save_toggle(&data);
    println!(
        "  세션 시작 시 미터 창을 {}.",
        if on_now {
            "띄웁니다"
        } else {
            "띄우지 않습니다"
        }
    );
    if on_now && daemon_pid() != 0 {
        let _ = fs::write(data_dir().join("overlay.show"), b"");
        println!("  실행 중인 미터 창을 다시 띄웁니다.");
    }
    0
}

fn cmd_update(args: &Args) -> i32 {
    let mut data = load_toggle();
    if args.rest.is_empty() {
        println!(
            "  자동 업데이트: {}",
            if data.get("auto_update") == Some(&json!(true)) {
                "켜짐"
            } else {
                "꺼짐"
            }
        );
        return 0;
    }
    match args.rest[0].as_str() {
        "on" | "off" => {
            data.as_object_mut()
                .unwrap()
                .insert("auto_update".into(), json!(args.rest[0] == "on"));
            save_toggle(&data);
            println!(
                "  자동 업데이트를 {}습니다.",
                if args.rest[0] == "on" { "켰습니" } else { "껐습니" }
            );
            0
        }
        "now" => {
            let (ok, msg) = install::update_package(true);
            println!("  {msg}");
            if ok.is_none() {
                1
            } else {
                0
            }
        }
        _ => 1,
    }
}

fn cmd_services() -> i32 {
    println!("TokenMeter 서비스");
    services_table();
    println!();
    println!("  설정을 고친 뒤에는 `tokenmeter doctor <서비스>` 로 파싱을 검증하세요.");
    0
}

fn cmd_doctor(args: &Args) -> i32 {
    let all = load_all_specs();
    let specs: Vec<_> = if args.rest.is_empty() {
        all
    } else {
        all.into_iter()
            .filter(|s| args.rest.iter().any(|n| n == &s.name))
            .collect()
    };
    if specs.is_empty() {
        println!("⚠ 검사할 서비스가 없습니다.");
        return 1;
    }
    if let Some(since) = flag(args, "since") {
        let Some(start) = day_start(since) else {
            println!("{}", crate::l10n!("--since takes YYYY-MM-DD", "--since 는 YYYY-MM-DD 입니다"));
            return 1;
        };
        let until = flag(args, "until").unwrap_or("9999-12-31");
        if day_start(until).is_none() {
            println!("{}", crate::l10n!("--until takes YYYY-MM-DD", "--until 은 YYYY-MM-DD 입니다"));
            return 1;
        }
        let out: Vec<Value> = specs.iter().map(|s| doctor_since(s, since, start, until)).collect();
        if on(args, "json") {
            println!("{}", if out.len() == 1 { out[0].clone() } else { Value::Array(out) });
        } else {
            for v in out {
                println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
            }
        }
        return 0;
    }
    if on(args, "json") {
        let report = load_report();
        println!(
            "{}",
            json!({
                "version": VERSION,
                "os": std::env::consts::OS,
                "native_meter": true,
                "native_hook": install::hook_bin().is_file(),
                "league": league::caption(),
                "share": share::on(),
                "services": specs.iter().map(doctor_service_json).collect::<Vec<_>>(),
                // 이유 글에는 사용자 설정의 값(루트 등)이 들 수 있어 자리 이름만 낸다(1절 개인정보).
                "skipped": report.skipped.iter().map(|(id, why)| {
                    json!({"service": id, "site": why.split(':').next().unwrap_or_default()})
                }).collect::<Vec<_>>(),
                "warnings": report.warnings,
                "overlaps": report.overlaps.iter().map(|[a, b]| json!([[a.0, a.1], [b.0, b.1]])).collect::<Vec<_>>(),
                "roots_from_dropped": report.roots_from_dropped,
                "oversized_state": Store { dir: data_dir().join("readers") }.oversized(),
            })
        );
        return 0;
    }
    println!("TokenMeter 설정 진단\n");
    for (i, spec) in specs.iter().enumerate() {
        if i > 0 {
            println!();
        }
        doctor_one(spec);
    }
    println!();
    println!("  리그    : {}", league::caption());
    println!("  공유    : {}", share::caption());
    0
}

fn short_path(path: &std::path::Path) -> String {
    let text = path.display().to_string();
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() && text.starts_with(&home) {
            return format!("~{}", &text[home.len()..]);
        }
    }
    text
}

/// 가장 최근 로그 파일들(최신순)과 그 안의 델타, 그리고 그 읽기의 수(레코드, match 탈락, 필드 적중).
/// prime()+poll() 은 데몬 시작용이라 명령이 도는 사이 새로 붙은 줄만 세서 늘 0건이 된다.
fn doctor_sample(spec: &ServiceSpec) -> (Vec<PathBuf>, Vec<TokenDelta>, ServiceReader) {
    let mut reader = ServiceReader::new(spec.clone());
    let mut files = reader.files();
    // 키를 한 번씩만 읽는다: 정렬 중에 mtime 이 바뀌면 sort_by_key 는 패닉할 수 있다.
    files.sort_by_cached_key(|p| std::cmp::Reverse(fs::metadata(p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)));
    let n = if spec.format == "json" { 40 } else { 1 };
    let deltas = files.iter().take(n).flat_map(|p| reader.read_file(p, true)).collect();
    (files, deltas, reader)
}

/// 있는 루트를 공개해도 되는 이름으로: 기본 어댑터의 글자 그대로 루트는 `short_path`, 나머지
/// (환경 변수·사용자 덮어쓰기)는 `<id>#<번호>`. `roots_from` 루트는 내지 않는다(1절).
fn public_roots(spec: &ServiceSpec) -> Vec<String> {
    let vars = Vars { root: None, ctx: &|_| None };
    root_templates(spec)
        .into_iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let paths = roots::expand(t, &vars).ok().flatten()?;
            let dir = paths.into_iter().find(|p| p.is_dir())?;
            Some(if is_static_builtin_root(&spec.name, t) {
                short_path(&dir)
            } else {
                format!("{}#{i}", spec.name)
            })
        })
        .collect()
}

fn has_roots_from(spec: &ServiceSpec) -> bool {
    !spec.roots_from.is_empty() || spec.sources.iter().any(|s| !s.roots_from.is_empty())
}

fn field_hits_json(reader: &ServiceReader) -> Value {
    let round = |r: f64| (r * 1000.0).round() / 1000.0;
    Value::Object(reader.field_hits().into_iter().map(|(k, r)| (k.to_string(), json!(round(r)))).collect())
}

fn doctor_service_json(spec: &ServiceSpec) -> Value {
    let roots = public_roots(spec);
    if roots.is_empty() && !has_roots_from(spec) {
        return json!({"name": spec.name, "ok": false, "warning": "no-log-roots", "verified": spec.verified});
    }
    let (files, deltas, reader) = doctor_sample(spec);
    if files.is_empty() {
        return json!({"name": spec.name, "ok": false, "warning": "no-log-files", "verified": spec.verified});
    }
    let tokens: i64 = deltas.iter().map(|d| d.total()).sum();
    let models: std::collections::BTreeSet<&str> =
        deltas.iter().filter(|d| !d.model.is_empty()).map(|d| pricing::public_model(&d.model)).collect();
    json!({
        "name": spec.name,
        "label": spec.label,
        "format": spec.format,
        "mode": spec.mode,
        "verified": spec.verified,
        "log_files": files.len(),
        // 로그 파일명·폴더명에는 세션 id 와 인코딩된 홈 경로가 들어 있다 — 루트만 보인다
        "roots": roots,
        "records": reader.stats.records,
        "dropped_by_match": reader.stats.dropped_by_match,
        "fields": field_hits_json(&reader),
        "deltas": deltas.len(),
        "tokens": tokens,
        "models": models,
        "ok": !deltas.is_empty(),
    })
}

/// `YYYY-MM-DD`의 로컬 자정(유닉스 초).
fn day_start(date: &str) -> Option<f64> {
    let mut it = date.splitn(3, '-').map(|p| p.parse::<u32>().ok());
    let (y, m, d) = (it.next()??, it.next()??, it.next()??);
    if date.len() != 10 || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(crate::history::mktime_local(y as i32, m, d, 0, 0, -1))
}

/// 기간 안에 바뀐 파일을 새 읽기 도구로 처음부터 읽어 레코드 시각의 로컬 날짜·공개 모델별로 모은다
/// (스펙 13절). 시각이 없는 레코드는 오늘로 친다. 경로와 내용은 내지 않는다.
fn doctor_since(spec: &ServiceSpec, since: &str, start: f64, until: &str) -> Value {
    let mut reader = ServiceReader::new(spec.clone());
    let deltas = reader.read_since(start);
    let now = now_secs();
    let mut days = serde_json::Map::new();
    for d in deltas.iter().filter(|d| !d.speed_only) {
        let date = local_date(if d.at > 0.0 { d.at } else { now });
        if date.as_str() < since || date.as_str() > until {
            continue;
        }
        let cost = match d.cost_usd {
            Some(c) if c.is_finite() && c > 0.0 => c,
            _ => pricing::cost_usd(&d.model, d.input_tokens, d.cache_read, d.cache_write, d.output_tokens),
        };
        let day = days.entry(date).or_insert_with(|| json!({}));
        let model = pricing::public_model(&d.model);
        for (k, v) in [
            ("input", d.input_tokens),
            ("cache_read", d.cache_read),
            ("cache_write", d.cache_write),
            ("output", d.output_tokens),
        ] {
            add_json(&mut day[k], v as f64);
            add_json(&mut day["models"][model][k], v as f64);
        }
        add_json(&mut day["calls"], d.calls as f64);
        add_json(&mut day["cost_usd"], cost);
    }
    let mut sorted: Vec<(String, Value)> = days.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    json!({
        "service": spec.name,
        "verified": spec.verified,
        "records": reader.stats.records,
        "dropped_by_match": reader.stats.dropped_by_match,
        "fields": field_hits_json(&reader),
        "days": Value::Object(sorted.into_iter().collect()),
    })
}

/// 숫자 칸에 더한다(없으면 0에서). 정수로 떨어지면 정수로 둔다.
fn add_json(slot: &mut Value, v: f64) {
    let sum = slot.as_f64().unwrap_or(0.0) + v;
    *slot = if sum.fract() == 0.0 && sum.abs() < 9e15 { json!(sum as i64) } else { json!(sum) };
}

fn doctor_one(spec: &ServiceSpec) {
    println!(
        "[{}] {}  (format={}, mode={}, key={})",
        spec.label,
        spec.name,
        spec.format,
        spec.mode,
        spec.key.as_str().unwrap_or("-")
    );
    if !spec.verified {
        println!("   {}", crate::l10n!("⚠ not verified against a real log", "⚠ 실제 로그로 검증되지 않았습니다"));
    }
    if public_roots(spec).is_empty() && !has_roots_from(spec) {
        println!(
            "   ⚠ 존재하는 로그 경로가 없습니다: {}",
            spec.roots.join(", ")
        );
        return;
    }
    let (files, deltas, reader) = doctor_sample(spec);
    if files.is_empty() {
        println!("   ⚠ patterns 에 맞는 로그 파일이 없습니다");
        return;
    }
    println!("   로그 파일 : {}", files[0].display());
    let tokens: i64 = deltas.iter().map(|d| d.total()).sum();
    println!(
        "   추출 델타 : {}건 · {} 토큰",
        deltas.len(),
        num(tokens)
    );
    let st = &reader.stats;
    println!(
        "   {}",
        crate::l10n!(
            "records   : {} read · {} dropped by match",
            "레코드    : {}건 읽음 · match 에서 {}건 떨어짐",
            st.records,
            st.dropped_by_match
        )
    );
    let hits: Vec<String> =
        reader.field_hits().into_iter().map(|(k, r)| format!("{k} {:.0}%", r * 100.0)).collect();
    if !hits.is_empty() {
        println!("   {}", crate::l10n!("fields    : {}", "필드 적중 : {}", hits.join(" · ")));
    }
    let models: std::collections::BTreeSet<_> = deltas.iter().map(|d| d.model.clone()).collect();
    println!(
        "   감지 모델 : {}   (기본값 {})",
        if models.is_empty() {
            "-".into()
        } else {
            models.into_iter().collect::<Vec<_>>().join(", ")
        },
        spec.default_model
    );
}

fn cmd_quota(args: &Args) -> i32 {
    let snap = if on(args, "cached") {
        quota::load()
    } else {
        quota::refresh(true)
    };
    if on(args, "json") {
        println!("{}", quota::public(&snap));
        return 0;
    }
    println!("TokenMeter 한도");
    let windows = snap.get("windows").and_then(Value::as_array).cloned().unwrap_or_default();
    let errors = snap.get("errors").and_then(Value::as_object).cloned().unwrap_or_default();
    if windows.is_empty() && errors.is_empty() {
        println!("  읽을 자격 증명이 없습니다 (Claude/Codex/Grok 로그인)");
        return 0;
    }
    let now = now_secs();
    let mut rows = Vec::new();
    for row in &windows {
        let used = row.get("used").and_then(Value::as_f64);
        let pct = if let Some(u) = used {
            format!("{:.0}%", u * 100.0)
        } else if let Some(r) = row.get("remaining_usd").and_then(Value::as_f64) {
            format!("${r:.2}")
        } else {
            "-".into()
        };
        rows.push(vec![
            row.get("title").and_then(Value::as_str).unwrap_or("-").into(),
            row.get("label").and_then(Value::as_str).unwrap_or("-").into(),
            pct,
            quota::reset_caption(row.get("resets_at").and_then(Value::as_f64), now),
            row.get("status").and_then(Value::as_str).unwrap_or("-").into(),
        ]);
    }
    if !rows.is_empty() {
        table(&["서비스", "창", "사용", "리셋", "상태"], &rows, &[]);
    }
    for (name, msg) in errors {
        println!("  {name}: {}", msg.as_str().unwrap_or(""));
    }
    0
}

fn num_of(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().filter(|n| n.is_finite()).unwrap_or(0.0),
        Some(Value::String(s)) => s.parse().ok().filter(|n: &f64| n.is_finite()).unwrap_or(0.0),
        _ => 0.0,
    }
}

fn public_totals(node: &Value) -> Value {
    let n = |k: &str| num_of(node.get(k));
    json!({
        "input_tokens": n("input_tokens") as i64,
        "cache_read": n("cache_read") as i64,
        "cache_write": n("cache_write") as i64,
        "output_tokens": n("output_tokens") as i64,
        "cost_usd": n("cost_usd"),
        "calls": n("calls") as i64,
        "cache_saved_usd": n("cache_saved_usd"),
    })
}

fn public_group(node: &Value, model: bool) -> Value {
    let mut out = serde_json::Map::new();
    let Some(obj) = node.as_object() else {
        return Value::Object(out);
    };
    for (name, value) in obj {
        let Some(rec) = value.as_object() else {
            continue;
        };
        let mut entry = serde_json::Map::new();
        entry.insert(
            "totals".into(),
            public_totals(rec.get("totals").unwrap_or(&Value::Null)),
        );
        if rec.contains_key("sessions") {
            entry.insert("sessions".into(), json!(num_of(rec.get("sessions")) as i64));
        }
        if rec.contains_key("last_seen") {
            entry.insert("last_seen".into(), json!(num_of(rec.get("last_seen"))));
        }
        if model {
            if let Some(v) = rec.get("vendor").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                entry.insert("vendor".into(), json!(v));
            }
        }
        out.insert(name.clone(), Value::Object(entry));
    }
    Value::Object(out)
}

fn public_days(node: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let Some(obj) = node.as_object() else {
        return Value::Object(out);
    };
    for (name, value) in obj {
        if name.len() == 10 && name.as_bytes().get(4) == Some(&b'-') && name.as_bytes().get(7) == Some(&b'-')
        {
            out.insert(name.clone(), public_totals(value));
        }
    }
    Value::Object(out)
}

fn public_endpoints(node: &Value) -> Value {
    let mut out = serde_json::Map::new();
    let Some(obj) = node.as_object() else {
        return Value::Object(out);
    };
    for (url, value) in obj {
        let Some(rec) = value.as_object() else {
            continue;
        };
        let label = board::endpoint_label(url);
        let entry = out.entry(label).or_insert_with(|| {
            json!({
                "totals": public_totals(&Value::Null),
                "sessions": 0,
                "last_seen": 0.0,
            })
        });
        let Some(dst) = entry.as_object_mut() else {
            continue;
        };
        let add = public_totals(rec.get("totals").unwrap_or(&Value::Null));
        if let Some(tot) = dst.get_mut("totals").and_then(Value::as_object_mut) {
            for k in ["input_tokens", "cache_read", "cache_write", "output_tokens", "calls"] {
                let a = tot.get(k).and_then(Value::as_i64).unwrap_or(0);
                let b = add.get(k).and_then(Value::as_i64).unwrap_or(0);
                tot.insert(k.into(), json!(a + b));
            }
            for k in ["cost_usd", "cache_saved_usd"] {
                let a = tot.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                let b = add.get(k).and_then(Value::as_f64).unwrap_or(0.0);
                tot.insert(k.into(), json!(a + b));
            }
        }
        dst.insert(
            "sessions".into(),
            json!(dst.get("sessions").and_then(Value::as_i64).unwrap_or(0) + num_of(rec.get("sessions")) as i64),
        );
        dst.insert(
            "last_seen".into(),
            json!(dst
                .get("last_seen")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .max(num_of(rec.get("last_seen")))),
        );
    }
    Value::Object(out)
}

fn public_snapshot(state: &Value) -> Value {
    let now = now_secs();
    let sessions: Vec<Value> = session_views(state, now)
        .into_iter()
        .filter(|r| r.live)
        .map(|r| {
            json!({
                "service": r.service, "project": r.project, "model": r.model,
                "attention": r.attention, "started_at": r.started_at,
                "last_seen": r.last_seen, "attention_at": r.attention_at,
                "ctx": r.ctx, "ctx_window": r.ctx_win,
            })
        })
        .collect();
    let today = state.get("today").filter(|v| v.is_object()).unwrap_or(&Value::Null);
    let total = state.get("total").filter(|v| v.is_object()).unwrap_or(&Value::Null);
    json!({
        "schema_version": 1, "type": "snapshot", "timestamp": now,
        "updated_at": num_of(state.get("updated_at")),
        "today": {
            "date": today.get("date").and_then(Value::as_str).unwrap_or(""),
            "totals": public_totals(today.get("totals").unwrap_or(&Value::Null)),
        },
        "total": {
            "started_at": num_of(total.get("started_at")),
            "last_seen": num_of(total.get("last_seen")),
            "sessions": num_of(total.get("sessions")) as i64,
            "totals": public_totals(total.get("totals").unwrap_or(&Value::Null)),
        },
        "days": public_days(state.get("days").unwrap_or(&Value::Null)),
        "projects": public_group(state.get("projects").unwrap_or(&Value::Null), false),
        "services": public_group(state.get("services").unwrap_or(&Value::Null), false),
        "models": public_group(state.get("models").unwrap_or(&Value::Null), true),
        "vendors": public_group(state.get("vendors").unwrap_or(&Value::Null), false),
        "plans": public_group(state.get("plans").unwrap_or(&Value::Null), false),
        "endpoints": public_endpoints(state.get("endpoints").unwrap_or(&Value::Null)),
        "sessions": sessions,
    })
}

fn cmd_status(args: &Args) -> i32 {
    let meter = Meter::load();
    let state = meter.status();
    if on(args, "json") {
        println!("{}", public_snapshot(&state));
        return 0;
    }
    if on(args, "sync") {
        board::sync(&state, true);
    }
    let total = state.pointer("/total/totals").cloned().unwrap_or(json!({}));
    let today = state.pointer("/today/totals").cloned().unwrap_or(json!({}));
    let pid = daemon_pid();
    println!("TokenMeter 상태");
    println!(
        "  데몬   : {}   ·   라이브 세션 {}개",
        if pid != 0 {
            format!("실행 중 (pid {pid})")
        } else {
            "꺼짐".into()
        },
        state.get("live_count").and_then(Value::as_u64).unwrap_or(0)
    );
    println!("  리그    : {}", league::caption());
    println!("  공유    : {}", share::caption());
    let unknown: Vec<String> = state
        .get("models")
        .and_then(Value::as_object)
        .map(|m| {
            m.keys()
                .filter(|name| !pricing::known(name))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if !unknown.is_empty() {
        let shown = unknown.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
        let extra = if unknown.len() > 5 {
            format!(" 외 {}개", unknown.len() - 5)
        } else {
            String::new()
        };
        println!("  ⚠ 가격표에 없는 모델: {shown}{extra}");
        println!("    default 단가로 추정 중입니다 — `tokenmeter price set <모델> --input .. --output ..`");
    }
    if tokens_of(&total) == 0 && state.get("live").and_then(Value::as_array).map(|a| a.is_empty()).unwrap_or(true)
    {
        println!();
        println!("  첫 세션 대기 중");
        activation();
        return 0;
    }
    println!(
        "  오늘   : {} 토큰 · {} 호출 · {}",
        num(tokens_of(&today)),
        num(today.get("calls").and_then(Value::as_i64).unwrap_or(0)),
        usd(fnum(today.get("cost_usd").unwrap_or(&json!(0))))
    );
    println!(
        "  누적   : {} 토큰 · {} 호출 · {}",
        num(tokens_of(&total)),
        num(total.get("calls").and_then(Value::as_i64).unwrap_or(0)),
        usd(fnum(total.get("cost_usd").unwrap_or(&json!(0))))
    );
    if on(args, "sync") || board::online() {
        let scope = flag(args, "scope").unwrap_or("today");
        let (entries, note) = board::board(&state, scope);
        println!();
        table(
            &["#", "핸들", "토큰", "비용", ""],
            &entries
                .iter()
                .take(10)
                .enumerate()
                .map(|(i, e)| {
                    vec![
                        (i + 1).to_string(),
                        e.handle.clone(),
                        num(e.tokens),
                        usd(e.cost_usd),
                        if e.me { "◀ 나" } else { "" }.into(),
                    ]
                })
                .collect::<Vec<_>>(),
            &[2, 3],
        );
        println!("  {note}");
    }
    0
}

fn cmd_team(args: &Args) -> i32 {
    let state = Meter::load().status();
    if on(args, "sync") {
        board::sync(&state, true);
    }
    let (entries, _) = board::team(&state);
    if on(args, "json") {
        println!(
            "{}",
            json!({
                "schema_version": 1, "type": "team", "timestamp": now_secs(),
                "members": entries.iter().map(|e| json!({
                    "handle": e.handle, "check": e.check, "working": e.working,
                    "waiting": e.waiting, "risk": e.risk, "cost_usd": e.cost_usd, "me": e.me
                })).collect::<Vec<_>>()
            })
        );
        return 0;
    }
    table(
        &["핸들", "확인", "작업", "대기", "위험", "오늘"],
        &entries
            .iter()
            .map(|e| {
                vec![
                    e.handle.clone(),
                    num(e.check),
                    num(e.working),
                    num(e.waiting),
                    num(e.risk),
                    usd(e.cost_usd),
                ]
            })
            .collect::<Vec<_>>(),
        &[1, 2, 3, 4, 5],
    );
    0
}

fn cmd_receipt(args: &Args) -> i32 {
    let state = Meter::load().state;
    let sessions = state.get("sessions").and_then(Value::as_object);
    let Some(rec) = sessions.and_then(|s| {
        s.values()
            .filter(|v| v.is_object())
            .max_by(|a, b| {
                fnum(a.get("last_seen").unwrap_or(&json!(0)))
                    .partial_cmp(&fnum(b.get("last_seen").unwrap_or(&json!(0))))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }) else {
        println!("영수증을 만들 세션이 없습니다.");
        return 1;
    };
    let totals = rec.get("totals").cloned().unwrap_or(json!({}));
    let fmt = flag(args, "format").unwrap_or("text");
    let data = json!({
        "schema_version": 1, "type": "receipt",
        "project": rec.get("project").and_then(Value::as_str).unwrap_or("(unknown)"),
        "service": rec.get("service").and_then(Value::as_str).unwrap_or(""),
        "model": rec.get("model").and_then(Value::as_str).unwrap_or(""),
        "totals": totals,
        "amount_usd": fnum(totals.get("cost_usd").unwrap_or(&json!(0))),
    });
    if fmt == "json" {
        println!("{data}");
        return 0;
    }
    println!("TokenMeter 영수증");
    println!(
        "{} · {} · {}",
        data["project"], data["service"], data["model"]
    );
    println!(
        "입력 {} · 출력 {} · {}",
        num(totals.get("input_tokens").and_then(Value::as_i64).unwrap_or(0)),
        num(totals.get("output_tokens").and_then(Value::as_i64).unwrap_or(0)),
        usd(fnum(totals.get("cost_usd").unwrap_or(&json!(0))))
    );
    0
}

fn cmd_price(args: &Args) -> i32 {
    let action = args.rest.first().map(String::as_str).unwrap_or("");
    let model = args.rest.get(1).cloned().unwrap_or_default();
    if action == "set" {
        if model.is_empty() {
            println!("  사용법: tokenmeter price set <모델> --input 3 --output 15");
            return 1;
        }
        let mut values = HashMap::new();
        for (name, key) in [
            ("input", "input"),
            ("cache-read", "cache_read"),
            ("cache-write", "cache_write"),
            ("output", "output"),
            ("window", "window"),
        ] {
            if let Some(v) = flag(args, name).and_then(|s| s.parse::<f64>().ok()) {
                values.insert(key.to_string(), v);
            }
        }
        if values.is_empty() {
            println!("  사용법: tokenmeter price set <모델> --input 3 --output 15");
            return 1;
        }
        let entry = pricing::set_price(&model, &values);
        println!(
            "  {model} 단가를 저장했습니다: {}",
            entry
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        return 0;
    }
    if action == "unset" {
        if model.is_empty() {
            println!("  사용법: tokenmeter price unset <모델>");
            return 1;
        }
        println!(
            "  {}",
            if pricing::unset_price(&model) {
                format!("{model} 오버라이드를 지웠습니다.")
            } else {
                format!("{model} 에 지정된 오버라이드가 없습니다.")
            }
        );
        return 0;
    }
    let state = Meter::load().state;
    let used: Vec<String> = state
        .get("models")
        .and_then(Value::as_object)
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();
    let models = if used.is_empty() {
        pricing::listed_models()
    } else {
        used
    };
    let rows: Vec<Vec<String>> = models
        .iter()
        .map(|m| {
            let p = pricing::prices_for(m);
            vec![
                m.clone(),
                format!("{}", p.input),
                format!("{}", p.cache_read),
                format!("{}", p.cache_write),
                format!("{}", p.output),
                if p.window > 0 { num(p.window) } else { "-".into() },
                if pricing::has_override(m) {
                    "사용자"
                } else if pricing::known(m) {
                    "기본"
                } else {
                    "⚠ 추정(default)"
                }
                .into(),
            ]
        })
        .collect();
    println!("모델 단가 (USD / 1M 토큰)");
    table(
        &["모델", "입력", "캐시읽기", "캐시쓰기", "출력", "컨텍스트", "출처"],
        &rows,
        &[1, 2, 3, 4, 5],
    );
    0
}

fn cmd_start(args: &Args) -> i32 {
    let meter = Meter::load();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let path = meter.add_live(
        flag(args, "service").unwrap_or("manual"),
        flag(args, "session-id").unwrap_or(""),
        flag(args, "project").unwrap_or(""),
        &cwd,
        flag(args, "model").unwrap_or(""),
        "manual",
    );
    println!("라이브 세션 등록: {}", path.display());
    if spawn_daemon() {
        println!("데몬을 새로 띄웠습니다.");
    } else if daemon_pid() != 0 {
        println!("데몬 이미 실행 중 (pid {})", daemon_pid());
    }
    0
}

fn cmd_stop(args: &Args) -> i32 {
    let meter = Meter::load();
    let service = flag(args, "service").unwrap_or("");
    let sid = flag(args, "session-id").unwrap_or("");
    let live = meter.status().get("live").and_then(Value::as_array).cloned().unwrap_or_default();
    let targets: Vec<_> = live
        .iter()
        .filter(|s| {
            (service.is_empty() || s.get("service").and_then(Value::as_str) == Some(service))
                && (sid.is_empty() || s.get("session_id").and_then(Value::as_str) == Some(sid))
        })
        .cloned()
        .collect();
    if targets.is_empty() {
        println!("해제할 라이브 세션이 없습니다.");
        return 0;
    }
    for s in targets {
        let svc = s.get("service").and_then(Value::as_str).unwrap_or("");
        let id = s.get("session_id").and_then(Value::as_str).unwrap_or("");
        meter.archive(svc, id);
        meter.remove_live(svc, id);
        println!("해제: {svc} / {id}");
    }
    0
}

fn cmd_watch(args: &Args) -> i32 {
    if on(args, "jsonl") {
        let mut meter = Meter::load();
        let mut prev = public_snapshot(&meter.status());
        println!("{}", prev);
        loop {
            thread::sleep(Duration::from_millis(500));
            meter.reload();
            let cur = public_snapshot(&meter.status());
            if cur.get("updated_at") != prev.get("updated_at") {
                println!("{cur}");
                prev = cur;
            }
        }
    }
    if daemon_pid() != 0 {
        println!(
            "✗ 데몬(pid {})이 이미 상태 파일을 쓰고 있습니다 — 먼저 멈추세요",
            daemon_pid()
        );
        return 1;
    }
    crate::daemon::run(true)
}

fn cmd_overlay() -> i32 {
    if daemon_pid() != 0 {
        let _ = fs::write(data_dir().join("overlay.show"), b"");
        println!("실행 중인 미터 창을 앞으로 가져옵니다.");
        return 0;
    }
    let meter = Meter::load();
    let rate = LiveRate::new();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(snapshot_from(
        &meter.status(),
        &rate,
    )));
    crate::overlay::run_window(shared, false)
}

fn cmd_reset(args: &Args) -> i32 {
    if !on(args, "yes") {
        println!("정말 초기화하려면 `--yes` 를 붙이세요.");
        return 1;
    }
    if daemon_pid() != 0 {
        println!(
            "✗ 데몬(pid {})이 실행 중이라 초기화가 곧 덮어써집니다 — 먼저 멈추세요",
            daemon_pid()
        );
        return 1;
    }
    Meter::load().reset_stats();
    println!("통계를 초기화했습니다. (이미 서버에 올라간 랭킹은 다음 동기화 때 덮어써집니다)");
    0
}

fn cmd_adapter(args: &Args) -> i32 {
    match args.rest.first().map(String::as_str) {
        Some("init") => {
            let name = args.rest.get(1).cloned().unwrap_or_default();
            if name.is_empty() || name.contains('/') || name.contains('\\') || name == "." || name == ".."
            {
                println!("✗ 서비스 이름은 경로 구분자가 없는 한 단어여야 합니다");
                return 1;
            }
            let Some(log) = flag(args, "log") else {
                println!("✗ --log 가 필요합니다");
                return 1;
            };
            let out = PathBuf::from(format!("{name}-adapter"));
            match init_adapter(&name, &PathBuf::from(log), &out) {
                Ok(msg) => {
                    println!("✓ {msg}");
                    0
                }
                Err(msg) => {
                    println!("✗ {msg}");
                    1
                }
            }
        }
        Some("check") => {
            let path = args.rest.get(1).cloned().unwrap_or_default();
            let (ok, msgs) = check_adapter(&PathBuf::from(path));
            for m in msgs {
                println!("{m}");
            }
            if ok {
                0
            } else {
                1
            }
        }
        _ => {
            println!("사용법: tokenmeter adapter init|check");
            1
        }
    }
}

fn cmd_league(args: &Args) -> i32 {
    match args.rest.first().map(String::as_str) {
        Some("login") => league::login(),
        Some("logout") => league::logout(),
        Some("open") => league::open_room(""),
        Some("join") => league::join_room(args.rest.get(1).map(String::as_str).unwrap_or("")),
        Some("leave") => league::leave(args.rest.get(1).map(String::as_str)),
        Some("close") => league::close_room(args.rest.get(1).map(String::as_str)),
        Some("match") => league::match_cmd(args.rest.get(1).map(String::as_str), flag(args, "minutes"), flag(args, "rule")),
        _ => league::show(),
    }
}

fn cmd_share(args: &Args) -> i32 {
    match args.rest.first().map(String::as_str) {
        None | Some("status") => {
            println!("  공유    : {}", share::caption());
            0
        }
        Some("on") => {
            share::set(true);
            println!("  익명 사용 통계를 켰습니다. 보낼 내용: tokenmeter share preview");
            0
        }
        Some("off") => {
            share::set(false);
            println!("  익명 사용 통계를 껐습니다. 이미 보낸 데이터까지 지우려면: tokenmeter account delete");
            0
        }
        Some("preview") => {
            println!("{}", crate::sync::preview(&crate::engine::Meter::load().state));
            0
        }
        _ => {
            println!("사용법: tokenmeter share [on|off|status|preview]");
            1
        }
    }
}

fn cmd_account(args: &Args) -> i32 {
    if args.rest.first().map(String::as_str) != Some("delete") {
        println!("사용법: tokenmeter account delete [--yes]");
        return 1;
    }
    // 무엇보다 먼저 공유를 끈다. 서버 호출 중에 데몬이 새 기기를 받거나 다시 보내지 않게.
    crate::share::set(false);
    let Some(token) = crate::server::device_token() else {
        crate::sync::forget();
        println!("  서버에 보낸 데이터가 없습니다. 공유를 껐습니다.");
        return 0;
    };
    if crate::league::has_auth() {
        println!("  You are logged in to Token League: this also deletes your league account, every linked device's data and your room memberships.");
    }
    if !on(args, "yes") {
        print!("  서버에 있는 이 기기의 사용 데이터를 모두 지웁니다. 계속할까요? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes" | "예") {
            println!("  취소했습니다. 공유는 꺼 두었습니다 · 켜기: tokenmeter share on");
            return 1;
        }
    }
    match crate::server::delete_account(&token) {
        Ok(()) | Err(crate::server::ApiError::Status(401, _)) => {
            crate::server::forget_device();
            crate::sync::forget();
            crate::league::forget();
            println!("  서버의 사용 데이터를 지우고 공유를 껐습니다.");
            0
        }
        Err(e) => {
            // 기기 토큰은 남겨 다시 시도할 수 있게 한다. 공유는 꺼진 채다.
            let next = if e == crate::server::ApiError::Upgrade { " tokenmeter update now 뒤 다시 실행하세요." } else { "" };
            println!("  지우지 못했습니다: {}. 공유는 껐습니다.{next}", api_reason(&e));
            1
        }
    }
}

/// 서버 오류를 사람이 읽는 말로.
fn api_reason(e: &crate::server::ApiError) -> String {
    use crate::server::ApiError;
    match e {
        ApiError::Offline => "서버에 닿지 못했습니다".into(),
        ApiError::Upgrade => "서버가 새 버전을 요구합니다".into(),
        ApiError::Off => "서버 주소 설정이 비어 있습니다".into(),
        ApiError::Status(code, err) if err.is_empty() => format!("서버 오류({code})"),
        ApiError::Status(code, err) => format!("서버 오류({code} {err})"),
    }
}

fn print_help() {
    println!("usage: tokenmeter [-h] <명령>");
    println!("TokenMeter {VERSION} — 에이전트 토큰 자동 측정 + 미터/랭킹 오버레이");
    println!(
        "명령: install uninstall on off meter update services doctor quota status team receipt price daemon start stop watch overlay reset adapter league share account"
    );
}

pub fn run(argv: &[String]) -> i32 {
    let args = parse(argv);
    if args.cmd == "--help"
        || args.cmd == "-h"
        || on(&args, "help")
        || args.rest.iter().any(|s| s == "-h" || s == "--help")
    {
        print_help();
        return 0;
    }
    if args.cmd == "--version" || on(&args, "version") {
        println!("tokenmeter {VERSION}");
        return 0;
    }
    match args.cmd.as_str() {
        "install" => cmd_install(&args),
        "uninstall" => cmd_uninstall(&args),
        "on" => toggle_measure(true, &flags(&args, "service")),
        "off" => toggle_measure(false, &flags(&args, "service")),
        "meter" => cmd_meter(&args),
        "update" => cmd_update(&args),
        "services" => cmd_services(),
        "doctor" => cmd_doctor(&args),
        "quota" => cmd_quota(&args),
        "status" => cmd_status(&args),
        "team" => cmd_team(&args),
        "receipt" => cmd_receipt(&args),
        "price" => cmd_price(&args),
        "start" => cmd_start(&args),
        "stop" => cmd_stop(&args),
        "watch" => cmd_watch(&args),
        "overlay" => cmd_overlay(),
        "reset" => cmd_reset(&args),
        "adapter" => cmd_adapter(&args),
        "league" => cmd_league(&args),
        "share" => cmd_share(&args),
        "account" => cmd_account(&args),
        "daemon" => crate::daemon::run(on(&args, "no-window") || on(&args, "no-overlay")),
        _ => {
            eprintln!("알 수 없는 명령: {}", args.cmd);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_help_and_status_flags() {
        let help = parse(&["--help".into()]);
        assert_eq!(help.cmd, "--help");
        let a = parse(&[
            "status".into(),
            "--scope".into(),
            "total".into(),
            "--json".into(),
        ]);
        assert_eq!(a.cmd, "status");
        assert_eq!(flag(&a, "scope"), Some("total"));
        assert!(on(&a, "json"));
    }

    #[test]
    fn public_snapshot_sanitizes_broken_totals() {
        let state = json!({
            "updated_at": {},
            "sessions": ["broken"],
            "today": {"date": "2026-08-14", "totals": ["broken"]},
            "total": ["broken"],
            "live": [],
        });
        let snap = public_snapshot(&state);
        assert_eq!(snap["type"], "snapshot");
        assert_eq!(snap["sessions"], json!([]));
        assert_eq!(snap["today"]["totals"]["input_tokens"], 0);
        assert_eq!(snap["today"]["date"], "2026-08-14");
    }

    #[test]
    fn public_snapshot_keeps_only_allowlisted_fields() {
        let (_g, _tmp) = crate::test_home("public");
        let now = now_secs();
        let secret_url = "https://gateway.secret.example/v1";
        let meta = "unexpected-nested-metadata";
        let state = json!({
            "updated_at": now,
            "today": {"date": "2026-08-14", "totals": {"output_tokens": 7, "meta": meta}},
            "total": {"totals": {"output_tokens": 7}, "meta": {"value": meta}},
            "days": {"2026-08-14": {"output_tokens": 7, "meta": meta}},
            "projects": {"api": {"totals": {"output_tokens": 7, "meta": meta}}},
            "services": {"codex": {"totals": {"output_tokens": 7}}},
            "models": {"gpt-5": {"totals": {"output_tokens": 7}, "vendor": "openai"}},
            "vendors": {"openai": {"totals": {"output_tokens": 7}}},
            "plans": {"api": {"totals": {"output_tokens": 7}}},
            "endpoints": {
                secret_url: {"totals": {"output_tokens": 7, "meta": meta}, "meta": meta},
                "https://api.openai.com/v1": {"totals": {"output_tokens": 3}}
            },
            "sessions": {
                "codex/secret-id": {
                    "service": "codex", "project": "api", "model": "gpt-5", "effort": "private-effort",
                    "cwd": "/Users/alice/secret/path", "prompt": "DO-NOT-LEAK",
                    "started_at": now - 10.0, "last_seen": now - 2.0, "ctx": 20, "ctx_win": 100,
                    "totals": {"output_tokens": 777777}
                },
                "codex/completed-secret-id": {
                    "service": "codex", "project": "completed-private", "model": "secret-model",
                    "effort": "private-completed-effort", "started_at": now - 20.0,
                    "last_seen": now - 15.0, "totals": {"output_tokens": 888888}
                }
            },
            "live": [{"service": "codex", "session_id": "secret-id", "project": "api",
                      "cwd": "/Users/alice/secret/path", "routing_env": {"OPENAI_BASE_URL": "secret"},
                      "attention": "working", "attention_at": now - 1.0}]
        });
        let out = public_snapshot(&state);
        let raw = out.to_string();
        assert_eq!((out["schema_version"].clone(), out["type"].clone()), (json!(1), json!("snapshot")));
        for private in ["secret-id", "/Users/alice", "routing_env", "private-effort", "completed-private",
                        "777777", "888888", secret_url, meta, "DO-NOT-LEAK", "prompt"] {
            assert!(!raw.contains(private), "{private} 가 공개 JSON 에 샜다");
        }
        let mut endpoints: Vec<_> = out["endpoints"].as_object().unwrap().keys().cloned().collect();
        endpoints.sort();
        assert_eq!(endpoints, ["api.openai.com", "self-hosted"], "사내 주소는 self-hosted 로 뭉친다");
        assert_eq!(out["endpoints"]["self-hosted"]["totals"]["output_tokens"], 7);
        assert_eq!(out["projects"]["api"]["totals"]["output_tokens"], 7);
        assert_eq!(out["models"]["gpt-5"]["vendor"], "openai");
        let sessions = out["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1, "종료된 세션은 공개 스냅샷에 없다");
        let mut fields: Vec<_> = sessions[0].as_object().unwrap().keys().cloned().collect();
        fields.sort();
        assert_eq!(fields, ["attention", "attention_at", "ctx", "ctx_window", "last_seen", "model", "project", "service", "started_at"]);
        assert_eq!((sessions[0]["ctx"].clone(), sessions[0]["ctx_window"].clone()), (json!(20), json!(100)));
    }

    #[test]
    fn account_delete_removes_server_data_and_the_local_device() {
        let (_g, _tmp) = crate::test_home("account-delete");
        let (url, seen) = crate::server::fake::serve(vec![(204, "")]);
        crate::server::fake::use_server(&url);
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("device.json"), r#"{"device_id":"d1","token":"tmd_x"}"#).unwrap();
        crate::share::set(true);
        assert_eq!(run(&["account".into(), "delete".into(), "--yes".into()]), 0);
        let req = seen.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(req.starts_with("DELETE /v1/account") && req.contains("Bearer tmd_x"), "{req}");
        assert!(!data_dir().join("device.json").exists() && !crate::share::on());
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn account_delete_without_a_token_still_turns_sharing_off() {
        let (_g, _tmp) = crate::test_home("account-no-token");
        let (url, seen) = crate::server::fake::serve(vec![(204, "")]);
        crate::server::fake::use_server(&url);
        crate::share::set(true);
        fs::write(data_dir().join("league-sync.json"), r#"{"synced_through":"2026-09-25T10"}"#).unwrap();
        assert_eq!(run(&args(&["account", "delete", "--yes"])), 0);
        assert!(!crate::share::on(), "보낸 게 없어도 공유는 끈다");
        assert!(!data_dir().join("league-sync.json").exists());
        assert!(seen.recv_timeout(std::time::Duration::from_millis(300)).is_err(), "서버에 묻지 않는다");
    }

    #[test]
    fn account_delete_failure_keeps_the_token_but_sharing_stays_off() {
        let (_g, _tmp) = crate::test_home("account-fail");
        let (url, seen) = crate::server::fake::serve(vec![(500, r#"{"error":"internal","message":"db"}"#)]);
        crate::server::fake::use_server(&url);
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("device.json"), r#"{"device_id":"d1","token":"tmd_x"}"#).unwrap();
        crate::share::set(true);
        assert_eq!(run(&args(&["account", "delete", "--yes"])), 1);
        assert!(seen.recv_timeout(std::time::Duration::from_secs(5)).unwrap().starts_with("DELETE /v1/account"));
        assert!(data_dir().join("device.json").exists(), "다시 시도할 수 있게 기기 토큰은 남긴다");
        assert!(!crate::share::on());
    }

    #[test]
    fn purge_deletes_server_data_before_wiping_the_device_token() {
        let (_g, _tmp) = crate::test_home("purge-server");
        let (url, seen) = crate::server::fake::serve(vec![(204, "")]);
        crate::server::fake::use_server(&url);
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("device.json"), r#"{"device_id":"d1","token":"tmd_x"}"#).unwrap();
        crate::share::set(true);
        assert_eq!(run(&args(&["uninstall", "--purge"])), 0);
        let req = seen.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(req.starts_with("DELETE /v1/account") && req.contains("Bearer tmd_x"), "{req}");
        assert!(!data_dir().exists());
    }

    #[test]
    fn purge_keeps_the_device_token_when_the_server_delete_fails() {
        let (_g, _tmp) = crate::test_home("purge-fail");
        let (url, seen) = crate::server::fake::serve(vec![(500, r#"{"error":"internal","message":"db"}"#)]);
        crate::server::fake::use_server(&url);
        fs::create_dir_all(data_dir()).unwrap();
        fs::write(data_dir().join("device.json"), r#"{"device_id":"d1","token":"tmd_x"}"#).unwrap();
        crate::share::set(true);
        assert_eq!(run(&args(&["uninstall", "--purge"])), 1);
        assert!(seen.recv_timeout(std::time::Duration::from_secs(5)).unwrap().starts_with("DELETE /v1/account"));
        assert!(data_dir().join("device.json").exists(), "서버 데이터를 지울 유일한 토큰은 남긴다");
        assert!(!crate::share::on());
    }

    #[test]
    fn unknown_league_commands_print_the_notice() {
        let (_g, _tmp) = crate::test_home("league-unknown");
        assert_eq!(run(&args(&["league", "rooms"])), 0);
        assert_eq!(run(&args(&["league"])), 0);
    }
}
