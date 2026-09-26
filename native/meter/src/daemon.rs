//! 워처 루프 + 선택형 오버레이. `--no-window` 면 창 없이 측정만 한다.

use crate::engine::{lock_file, pid_file, Meter};
use crate::live_rate::LiveRate;
use crate::overlay::{snapshot_from_scale, SharedMeter};
use crate::watch::{load_runtime_config, ServiceReader};
use std::collections::HashMap;
use std::fs;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tokenmeter_hook::data_dir;

static STOP: AtomicBool = AtomicBool::new(false);

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
    let mut readers: Vec<ServiceReader> =
        runtime.specs.into_iter().map(ServiceReader::new).collect();
    if readers.is_empty() {
        eprintln!("[TokenMeter] 감시할 서비스가 없습니다");
        let _ = fs::remove_file(pid_file());
        return 1;
    }
    for reader in &mut readers {
        reader.prime();
    }
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
        while !STOP.load(Ordering::Relaxed) {
            if last_poll.elapsed() >= poll_every {
                for reader in &mut readers {
                    for delta in reader.poll() {
                        meter.ingest(delta);
                    }
                }
                last_poll = Instant::now();
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
            crate::sync::tick(&status);
            if crate::board::online() && last_board.elapsed().as_secs_f64() >= crate::board::sync_seconds()
            {
                crate::board::sync(&status, false);
                last_board = Instant::now();
            }
            thread::sleep(tick);
        }
        crate::sync::flush(&meter.state);
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
