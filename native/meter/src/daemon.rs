//! 워처 루프 + 선택형 오버레이. `--no-window` 면 창 없이 측정만 한다.

use crate::engine::{lock_file, pid_file, Meter};
use crate::live_rate::LiveRate;
use crate::overlay::{snapshot_from_scale, SharedMeter};
use crate::watch::checkpoint::Store;
use crate::watch::{load_report, load_runtime_config, now_secs, ServiceReader};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokenmeter_hook::data_dir;

static STOP: AtomicBool = AtomicBool::new(false);
/// 바뀐 것이 있으면 이만큼에 한 번 state.json을 커밋한다(스펙 4.2).
const COMMIT_EVERY: Duration = Duration::from_secs(30);
/// `.keys` 압축 주기(스펙 4.1).
/// 활동이 없어도 `perf` 칸을 이만큼마다 state.json에 쓴다.
const PERF_EVERY: Duration = Duration::from_secs(300);
const COMPACT_EVERY: Duration = Duration::from_secs(3600);
/// 문턱 여유와 밀린 기록 상한(스펙 4.4).
const GATE_SLACK: f64 = 600.0;
const BACKLOG_SECS: f64 = 7.0 * 86400.0;

pub fn stopping() -> bool {
    STOP.load(Ordering::Relaxed)
}

pub fn request_stop() {
    STOP.store(true, Ordering::Relaxed);
}

pub fn run(no_window: bool) -> i32 {
    STOP.store(false, Ordering::Relaxed);
    let _ = fs::create_dir_all(data_dir());
    let _ = fs::create_dir_all(tokenmeter_hook::live_dir());
    let runtime = load_runtime_config();
    if !runtime.settings.enabled {
        eprintln!("[TokenMeter] 측정이 꺼져 있습니다");
        return 0;
    }
    if already_running() {
        eprintln!("[TokenMeter] 데몬이 이미 실행 중입니다");
        return 0;
    }
    let _ = fs::write(pid_file(), format!("{}", std::process::id()));
    let _ = fs::remove_file(lock_file());
    crate::league::cleanup_legacy();

    let mut meter = Meter::load();
    meter.set_session_history(runtime.settings.session_history);
    let mut rate = LiveRate::new();
    for (id, why) in load_report().skipped {
        eprintln!("[TokenMeter] skipped service {id}: {why}");
    }
    let mut readers: Vec<ServiceReader> =
        runtime.specs.into_iter().map(ServiceReader::new).collect();
    if readers.is_empty() {
        eprintln!("[TokenMeter] 감시할 서비스가 없습니다");
        let _ = fs::remove_file(pid_file());
        return 1;
    }
    // 뜰 때(스펙 4.2): 복구 → 읽기 상태 되살리기 → .keys 압축 → 처음 읽기 → 커밋
    let store = Store { dir: data_dir().join("readers") };
    let seq = meter.next_seq() - 1;
    store.recover(seq);
    let toggle = crate::cli::load_toggle();
    let last_commit_at = meter.state.get("updated_at").and_then(Value::as_f64).unwrap_or(0.0);
    let now = now_secs();
    for reader in &mut readers {
        let id = reader.spec.name.clone();
        let keys = store.keys(&id, seq);
        let _ = store.compact(&id, &keys, seq);
        let file = store.load(&id);
        // 처음 실행(readers 없음, 측정을 다시 켬)이면 문턱은 지금이다(4.3)
        let gate = if file.is_some() {
            (last_commit_at - GATE_SLACK)
                .max(now - BACKLOG_SECS)
                .max(measure_since(&toggle, &id))
        } else {
            now
        };
        reader.set_gate(gate);
        reader.restore(file, keys);
    }
    let mut staged: HashMap<String, Vec<u8>> = HashMap::new();
    commit(&mut meter, &mut readers, &store, &mut staged);
    eprintln!(
        "[TokenMeter] 네이티브 데몬 시작 pid={} 서비스=[{}]",
        std::process::id(),
        readers
            .iter()
            .map(|r| r.spec.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    // `tokenmeter update on` 이면 하루 한 번 정식 릴리스로 바이너리를 바꾼다. 측정은 기다리지 않는다.
    thread::spawn(|| {
        let (_, msg) = crate::install::update_package(false);
        if !msg.is_empty() {
            eprintln!("[TokenMeter] {msg}");
        }
    });

    install_signals();
    let shared: SharedMeter = Arc::new(Mutex::new(Default::default()));
    let shared_loop = shared.clone();
    let tick = Duration::from_millis(200);
    let poll_every = Duration::from_secs_f64(runtime.settings.poll_seconds);
    let idle_limit = Duration::from_secs_f64(runtime.settings.idle_exit_minutes * 60.0);
    let live_ttl = Duration::from_secs_f64(runtime.settings.live_ttl_hours * 3600.0);
    let full_scale = runtime.settings.full_scale;
    let notify_attention = runtime.settings.attention_notify;

    let watch = thread::spawn(move || {
        let mut last = Instant::now();
        let mut last_poll = Instant::now() - poll_every;
        let mut last_prune = Instant::now();
        let mut idle_since: Option<Instant> = None;
        let mut notified_at: HashMap<String, f64> = HashMap::new();
        let mut last_quota_check = Instant::now() - Duration::from_secs(180);
        let mut last_board = Instant::now() - Duration::from_secs(60);
        let mut last_commit = Instant::now();
        let mut last_compact = Instant::now();
        let mut live_seen = HashSet::new();
        new_sessions(&mut live_seen);
        while !STOP.load(Ordering::Relaxed) {
            if last_poll.elapsed() >= poll_every {
                let (started, at) = (Instant::now(), now_secs());
                // 새 세션은 새 프로젝트 디렉터리일 수 있다: 그 서비스를 이번 폴에 훑는다(스펙 14절)
                for svc in new_sessions(&mut live_seen) {
                    readers
                        .iter_mut()
                        .filter(|r| tokenmeter_hook::safe_name(&r.spec.name) == svc)
                        .for_each(ServiceReader::request_scan);
                }
                let mut full = false;
                for reader in &mut readers {
                    for delta in reader.poll_at(at) {
                        meter.ingest(delta);
                    }
                    full |= reader.last_full;
                }
                meter.record_poll(at, started.elapsed().as_secs_f64() * 1000.0, full);
                last_poll = Instant::now();
            }
            // ponytail: 델타 없이 배우기만 한 읽기 상태는 다음 커밋까지 기다린다. 그사이 죽으면 다시 배운다.
            let rolled = meter.state.pointer("/hour/h") != meter.committed().pointer("/hour/h");
            let since = last_commit.elapsed();
            if (meter.dirty() && (rolled || since >= COMMIT_EVERY)) || since >= PERF_EVERY {
                commit(&mut meter, &mut readers, &store, &mut staged);
                last_commit = Instant::now();
                if last_compact.elapsed() >= COMPACT_EVERY {
                    // 커밋 바로 뒤라 메모리의 키는 모두 커밋됐다
                    let seq = meter.next_seq() - 1;
                    for reader in &readers {
                        let _ = store.compact(&reader.spec.name, &reader.live_keys(), seq);
                    }
                    last_compact = Instant::now();
                }
            }
            let status = meter.status();
            if notify_attention {
                for row in crate::attention::session_views(&status, crate::watch::now_secs()) {
                    if row.attention == "check"
                        && row.attention_at > notified_at.get(&row.key).copied().unwrap_or(0.0)
                    {
                        notified_at.insert(row.key, row.attention_at);
                        notify("TokenMeter", &format!("{} · 확인 필요", row.project));
                    }
                }
            }
            if data_dir().join("reset.request").exists() {
                let _ = fs::remove_file(data_dir().join("reset.request"));
                meter.reset_stats();
                rate = LiveRate::new();
            }
            let incoming = rate.update(&status);
            let dt = last.elapsed().as_secs_f64().max(0.001);
            last = Instant::now();
            rate.tick(dt);
            if let Ok(mut snap) = shared_loop.lock() {
                *snap = snapshot_from_scale(&status, &rate, full_scale);
                snap.incoming = incoming;
            }
            let live = status
                .get("live_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let room_open = crate::league::room_open();
            if live > 0 || room_open || idle_limit.is_zero() {
                idle_since = None;
            } else if idle_since.is_none() {
                idle_since = Some(Instant::now());
            } else if idle_since
                .map(|at| at.elapsed() >= idle_limit)
                .unwrap_or(false)
            {
                STOP.store(true, Ordering::Relaxed);
                break;
            }
            if last_prune.elapsed() >= Duration::from_secs(300) {
                prune_live(live_ttl);
                last_prune = Instant::now();
            }
            if last_quota_check.elapsed() >= Duration::from_secs(180) {
                let _ = crate::quota::refresh(false);
                last_quota_check = Instant::now();
            }
            crate::league::tick(&status, rate.rate);
            crate::sync::tick(meter.committed());
            if crate::board::online() && last_board.elapsed().as_secs_f64() >= crate::board::sync_seconds()
            {
                crate::board::sync(&status, false);
                last_board = Instant::now();
            }
            thread::sleep(tick);
        }
        commit(&mut meter, &mut readers, &store, &mut staged);
        crate::sync::flush(meter.committed());
        let _ = fs::remove_file(pid_file());
    });

    if no_window {
        while !STOP.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(200));
        }
    } else if let Err(err) = crate::overlay::run_overlay_hidden(shared, !runtime.settings.overlay_auto) {
        if crate::overlay::OVERLAY_STARTED.load(Ordering::Relaxed) {
            // 떠 있던 창이 끊겼으면(컴포지터 종료 등) 끝내서 다음 훅이 창 있는 데몬을 다시 띄우게 한다.
            eprintln!("[TokenMeter] 오버레이 실패: {err}");
        } else {
            // 화면이 없으면(헤드리스 리눅스·SSH) 훅이 띄운 데몬도 창 없이 측정을 이어 간다.
            // 창을 부르면(overlay.show) 끝내서, 화면 있는 다음 훅이 창 있는 데몬을 띄우게 한다.
            eprintln!("[TokenMeter] 오버레이 실패, 창 없이 측정합니다: {err}");
            let show = data_dir().join("overlay.show");
            let _ = fs::remove_file(&show);
            while !STOP.load(Ordering::Relaxed) && !show.exists() {
                thread::sleep(Duration::from_millis(200));
            }
        }
    }
    STOP.store(true, Ordering::Relaxed);
    let _ = watch.join();
    0
}

/// 커밋 N(스펙 4.2): 바뀐 서비스마다 `stage` → `state.json`(커밋 지점) → `finish`.
/// `staged`는 서비스마다 마지막으로 커밋한 파일 항목이다. 같고 새 키가 없으면 쓰지 않는다.
fn commit(meter: &mut Meter, readers: &mut [ServiceReader], store: &Store, staged: &mut HashMap<String, Vec<u8>>) {
    let seq = meter.next_seq();
    let mut done = Vec::new();
    for reader in readers.iter_mut() {
        reader.prune_keys();
        let (mut file, keys) = reader.export();
        let files = serde_json::to_vec(&file.files).unwrap_or_default();
        if keys.is_empty() && staged.get(&reader.spec.name) == Some(&files) {
            continue;
        }
        file.seq = seq;
        let keys_len = || fs::metadata(store.dir.join(format!("{}.keys", reader.spec.name))).map_or(0, |m| m.len());
        let before = keys_len();
        if let Err(err) = store.stage(&reader.spec.name, &file, &keys) {
            eprintln!("{}", crate::l10n!("[TokenMeter] failed to save readers/{}: {}", "[TokenMeter] readers/{} 저장 실패: {}", reader.spec.name, err));
            continue;
        }
        let next = fs::metadata(store.dir.join(format!("{}.next.json", reader.spec.name))).map_or(0, |m| m.len());
        meter.add_write_bytes(next + keys_len().saturating_sub(before));
        done.push((reader.spec.name.clone(), files));
    }
    if let Err(err) = meter.commit(seq) {
        eprintln!("{}", crate::l10n!("[TokenMeter] failed to save state.json: {}", "[TokenMeter] state.json 저장 실패: {}", err));
        return;
    }
    for (id, files) in done {
        let _ = store.finish(&id);
        staged.insert(id, files);
    }
}

/// `toggle.json`의 `measure_since`(`"*"` = 전체, 아니면 서비스 id)에서 큰 값(4.4).
fn measure_since(toggle: &Value, id: &str) -> f64 {
    let book = toggle.get("measure_since");
    ["*", id]
        .iter()
        .filter_map(|k| book?.get(k)?.as_f64())
        .fold(0.0, f64::max)
}

/// `live/<서비스>__*.json` 중 지난번에 없던 파일의 서비스(`safe_name`). `seen`은 지금 목록으로 바꾼다.
fn new_sessions(seen: &mut HashSet<String>) -> Vec<String> {
    let names: HashSet<String> = fs::read_dir(tokenmeter_hook::live_dir())
        .map(|d| d.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();
    let mut out: Vec<String> = names
        .iter()
        .filter(|n| n.ends_with(".json") && !seen.contains(*n))
        .filter_map(|n| n.split_once("__").map(|(svc, _)| svc.to_string()))
        .collect();
    out.sort();
    out.dedup();
    *seen = names;
    out
}

fn prune_live(ttl: Duration) {
    if ttl.is_zero() {
        return;
    }
    let Ok(entries) = fs::read_dir(tokenmeter_hook::live_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("json") {
            continue;
        }
        let stale = path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| std::time::SystemTime::now().duration_since(mtime).ok())
            .map(|age| age >= ttl)
            .unwrap_or(false);
        if stale {
            let _ = fs::remove_file(path);
        }
    }
}

pub fn notify(title: &str, message: &str) {
    #[cfg(target_os = "macos")]
    {
        let clean = |value: &str| {
            value
                .chars()
                .filter(|c| !matches!(c, '"' | '\\' | '\n'))
                .collect::<String>()
        };
        let _ = Command::new("osascript")
            .args([
                "-e",
                &format!(
                    "display notification \"{}\" with title \"{}\"",
                    clean(message),
                    clean(title)
                ),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("notify-send")
            .args([title, message])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

fn already_running() -> bool {
    let Ok(text) = fs::read_to_string(pid_file()) else {
        return false;
    };
    let Ok(pid) = text.trim().parse::<i32>() else {
        return false;
    };
    if pid <= 0 || pid == std::process::id() as i32 {
        return false;
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output();
    match out {
        Ok(o) => tokenmeter_hook::is_live_daemon_command(&String::from_utf8_lossy(&o.stdout)),
        Err(_) => false,
    }
}

fn install_signals() {
    let _ = ctrlc::set_handler(|| STOP.store(true, Ordering::Relaxed));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_live_file_names_its_service_once() {
        let (_g, _tmp) = crate::test_home("live-new");
        let live = tokenmeter_hook::live_dir();
        fs::create_dir_all(&live).unwrap();
        fs::write(live.join("codex__old.json"), "{}").unwrap();
        let mut seen = HashSet::new();
        new_sessions(&mut seen);
        fs::write(live.join("claude-code__s1.json"), "{}").unwrap();
        fs::write(live.join("claude-code__s2.json"), "{}").unwrap();
        assert_eq!(new_sessions(&mut seen), ["claude-code"]);
        assert!(new_sessions(&mut seen).is_empty());
    }
}
