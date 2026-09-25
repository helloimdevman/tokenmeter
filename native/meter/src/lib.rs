pub mod adapter;
pub mod attention;
pub mod board;
pub mod cli;
pub mod daemon;
pub mod engine;
pub mod history;
pub mod i18n;
pub mod install;
pub mod league;
pub mod live_rate;
pub mod overlay;
pub mod pixel_faces;
pub mod pricing;
pub mod quota;
pub mod server;
pub mod share;
pub mod sync;
pub mod watch;

pub const VERSION: &str = "0.1.0";

#[cfg(test)]
pub(crate) static TEST_HOME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// HOME·상태·설정을 새 임시 디렉터리로 돌린다. 실제 사용자 파일은 건드리지 않는다.
#[cfg(test)]
pub(crate) fn test_home(tag: &str) -> (std::sync::MutexGuard<'static, ()>, std::path::PathBuf) {
    let guard = TEST_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = std::env::temp_dir().join(format!(
        "tokenmeter-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    for (var, dir) in [
        ("HOME", "home"),
        ("TOKENMETER_HOME", "state"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "xdg-state"),
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::env::set_var(var, root.join(dir));
    }
    (guard, root)
}

pub use attention::{attention_counts, attention_label, session_views, SessionView};
pub use engine::Meter;
pub use live_rate::LiveRate;
pub use overlay::{compact_num, ctx_status_caption, gauge_target, DEFAULT_FULL_SCALE};
pub use watch::{ServiceReader, ServiceSpec, TokenDelta};

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use watch::MatchWant;

    fn tmp() -> PathBuf {
        let p = std::env::temp_dir().join(format!("tokenmeter-meter-{}", std::process::id()));
        let _ = fs::create_dir_all(&p);
        p
    }

    #[test]
    fn attention_maps_working_waiting_check_and_session_end() {
        let now = 10_000.0;
        let state = json!({
            "sessions": {
                "claude-code/a": {
                    "service": "claude-code", "project": "api", "last_seen": now - 5.0,
                    "totals": {"output_tokens": 10}
                }
            },
            "live": [
                {"service": "claude-code", "session_id": "a", "attention": "working",
                 "attention_at": now - 1.0, "event": "UserPromptSubmit"},
                {"service": "codex", "session_id": "b", "attention": "check",
                 "attention_at": now - 1.0, "event": "PermissionRequest", "project": "web"},
                {"service": "opencode", "session_id": "c", "attention": "waiting",
                 "attention_at": now - 40.0, "event": "Stop", "project": "docs"}
            ]
        });
        let rows: Vec<_> = session_views(&state, now);
        let by: std::collections::HashMap<_, _> =
            rows.into_iter().map(|r| (r.key.clone(), r)).collect();
        assert_eq!(by["claude-code/a"].attention, "working");
        assert_eq!(attention_label(&by["claude-code/a"].attention), "작업");
        assert_eq!(by["codex/b"].attention, "check");
        assert_eq!(attention_label(&by["codex/b"].attention), "확인");
        assert_eq!(by["opencode/c"].attention, "waiting");
        assert_eq!(attention_label(&by["opencode/c"].attention), "대기");

        let after_end = json!({
            "sessions": {
                "claude-code/a": {"service": "claude-code", "last_seen": now - 5.0, "totals": {"output_tokens": 10}}
            },
            "live": []
        });
        let done = session_views(&after_end, now);
        assert_eq!(done[0].attention, "done");
        assert_eq!(attention_label("done"), "종료");
        assert!(!done[0].live);
    }

    #[test]
    fn stop_event_does_not_stay_check() {
        let now = 50.0;
        let state = json!({
            "sessions": {},
            "live": [{"service": "claude-code", "session_id": "s", "attention": "check",
                      "attention_at": now, "event": "Stop"}]
        });
        let rows = session_views(&state, now);
        assert_eq!(rows[0].attention, "waiting");
    }

    #[test]
    fn live_rate_rises_on_output_then_decays_to_zero() {
        let mut rate = LiveRate::new();
        let s0 = json!({"total": {"totals": {"output_tokens": 0}}, "sessions": {}});
        assert_eq!(rate.update(&s0), 0);
        let s1 = json!({"total": {"totals": {"output_tokens": 400}}, "sessions": {
            "claude-code/a": {"totals": {"output_tokens": 400}, "sub_output_tokens": 0}
        }});
        assert!(rate.update(&s1) > 0);
        assert!(rate.rate > 0.0);
        for _ in 0..400 {
            rate.tick(1.0);
        }
        assert_eq!(rate.rate, 0.0);
    }

    #[test]
    fn cjk_font_file_exists_for_korean_overlay() {
        let bundled = include_bytes!("../fonts/Pretendard-Regular.otf").len() > 100_000;
        let system = crate::overlay::cjk_font_candidates()
            .into_iter()
            .any(|(path, _)| std::path::Path::new(path).is_file());
        assert!(bundled || system, "한글 글리프가 있는 폰트가 없다");
    }

    #[test]
    fn project_and_day_rows_read_existing_state_shape() {
        let state = json!({
            "projects": {
                "work/api": {"totals": {"input_tokens": 10, "output_tokens": 20, "cache_read": 0, "cache_write": 0}, "last_seen": 9.0},
                "work/web": {"totals": {"input_tokens": 1, "output_tokens": 2, "cache_read": 0, "cache_write": 0}, "last_seen": 3.0}
            },
            "days": {
                "2026-08-22": {"input_tokens": 5, "output_tokens": 7, "cache_read": 1, "cache_write": 1, "cost_usd": 1.25},
                "2026-08-21": {"input_tokens": 1, "output_tokens": 1, "cache_read": 0, "cache_write": 0, "cost_usd": 0.1}
            },
            "sessions": {},
            "live": [
                {"service": "claude-code", "session_id": "a", "attention": "working", "attention_at": 99.0}
            ]
        });
        let projects = crate::overlay::project_rows(&state);
        assert_eq!(projects[0].0, "work/api");
        assert_eq!(projects[0].1, 30);
        let days = crate::overlay::day_rows(&state);
        assert_eq!(days[0].0, "2026-08-22");
        assert_eq!(days[0].1, 14);
        let views = session_views(&state, 100.0);
        assert_eq!(crate::overlay::filter_session_rows(&views, "live").len(), 1);
        assert_eq!(
            crate::overlay::filter_session_rows(&views, "archive").len(),
            0
        );
        assert_eq!(crate::overlay::filter_session_rows(&views, "all").len(), 1);
    }

    #[test]
    fn overlay_contract_pins_snapshot_state_and_layout() {
        assert_eq!(DEFAULT_FULL_SCALE, 3000.0);
        assert_eq!(crate::overlay::SEGMENTS, 28);
        assert_eq!(crate::overlay::BASE_W, 340.0);
        let now = 100.0;
        let state = json!({
            "total": {"totals": {
                "input_tokens": 10, "output_tokens": 20, "cache_read": 3, "cache_write": 1,
                "cost_usd": 0.1, "cache_saved_usd": 0.0, "calls": 1
            }, "sessions": 1},
            "today": {"date": "2026-09-16", "totals": {
                "input_tokens": 4, "output_tokens": 8, "cache_read": 1, "cache_write": 0,
                "cost_usd": 0.05, "cache_saved_usd": 0.0, "calls": 1
            }},
            "projects": {},
            "days": {},
            "sessions": {},
            "live": [{"service": "claude-code", "session_id": "a",
                      "attention": "working", "attention_at": now}]
        });
        let snap = crate::overlay::snapshot_from(&state, &LiveRate::new());
        assert_eq!(snap.input_tokens, 4);
        assert_eq!(snap.output_tokens, 8);
        assert_eq!(snap.today_output, 8);
        assert_eq!(snap.total_output, 20);
        assert_eq!(snap.sessions.len(), 1);
        assert!(snap.full_scale >= 1.0);
        for key in ["total", "today", "sessions", "live", "projects", "days"] {
            assert!(state.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn compact_num_and_ctx_match_existing_meter() {
        assert_eq!(compact_num(12_345.0), "12.3k");
        assert_eq!(compact_num(1_234_567.0), "1.2M");
        assert_eq!(ctx_status_caption(0.5, false), "미상");
        assert_eq!(ctx_status_caption(0.95, true), "95% · 높음");
        assert_eq!(ctx_status_caption(0.31, true), "31%");
    }

    #[test]
    fn gauge_is_monotonic_and_clamped() {
        assert_eq!(gauge_target(0.0, DEFAULT_FULL_SCALE), 0.0);
        assert_eq!(
            gauge_target(DEFAULT_FULL_SCALE * 99.0, DEFAULT_FULL_SCALE),
            1.0
        );
        assert!(gauge_target(10.0, DEFAULT_FULL_SCALE) < gauge_target(50.0, DEFAULT_FULL_SCALE));
        assert!(
            gauge_target(50.0, DEFAULT_FULL_SCALE)
                < gauge_target(DEFAULT_FULL_SCALE, DEFAULT_FULL_SCALE)
        );
    }

    #[test]
    fn jsonl_watch_emits_output_delta_then_meter_counts() {
        let dir = tmp().join("logs");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s.jsonl");
        fs::write(&path, "").unwrap();
        let spec = ServiceSpec {
            name: "claude-code".into(),
            roots: vec![dir.to_string_lossy().into_owned()],
            patterns: vec!["**/*.jsonl".into()],
            format: "jsonl".into(),
            match_fields: [("type".into(), MatchWant::One("assistant".into()))].into(),
            mode: "delta".into(),
            key: Some("uuid".into()),
            fields: [
                ("input".into(), Some("message.usage.input_tokens".into())),
                (
                    "cache_read".into(),
                    Some("message.usage.cache_read_input_tokens".into()),
                ),
                (
                    "cache_write".into(),
                    Some("message.usage.cache_creation_input_tokens".into()),
                ),
                ("output".into(), Some("message.usage.output_tokens".into())),
            ]
            .into(),
            context: [
                ("cwd".into(), "cwd".into()),
                ("model".into(), "message.model".into()),
                ("session".into(), "sessionId".into()),
            ]
            .into(),
            default_model: "claude-sonnet".into(),
            vendor: "anthropic".into(),
            ..ServiceSpec::default()
        };
        let mut reader = ServiceReader::new(spec);
        reader.prime();
        assert!(reader.poll().is_empty());
        let rec = json!({
            "type": "assistant",
            "uuid": "u-1",
            "cwd": "/Users/dev/projects/tokenmeter",
            "sessionId": "sess-1",
            "message": {
                "model": "claude-opus-5",
                "usage": {
                    "input_tokens": 2,
                    "cache_creation_input_tokens": 10,
                    "cache_read_input_tokens": 5,
                    "output_tokens": 80
                }
            }
        });
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f, "{rec}").unwrap();
        drop(f);
        let deltas = reader.poll();
        assert_eq!(deltas.len(), 1, "{deltas:?}");
        assert_eq!(deltas[0].output_tokens, 80);
        assert_eq!(deltas[0].session, "sess-1");
        let mut second = rec.clone();
        second["uuid"] = json!("u-2");
        second["message"]["usage"]["output_tokens"] = json!(10);
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        write!(f, "{second}").unwrap();
        drop(f);
        assert!(
            reader.poll().is_empty(),
            "완결되지 않은 마지막 줄을 소비하면 안 된다"
        );
        let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(f).unwrap();
        drop(f);
        let completed = reader.poll();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].output_tokens, 10);
        let mut meter = Meter::new();
        meter.ingest(deltas.into_iter().next().unwrap());
        let mut rate = LiveRate::new();
        rate.update(&meter.state);
        let after = meter.state.clone();
        let mut totals = after.clone();
        if let Some(t) = totals.pointer_mut("/total/totals/output_tokens") {
            *t = json!(80);
        }
        // first update sets baseline 80, second with more output raises rate
        let mut st = meter.state.clone();
        if let Some(value) = st.pointer_mut("/total/totals/output_tokens") {
            *value = json!(160);
        }
        assert!(rate.update(&st) > 0);
        assert!(rate.rate > 0.0);
        let views =
            session_views(
                &{
                    let mut s = meter.state.clone();
                    s.as_object_mut().unwrap().insert("live".into(), json!([{
                "service": "claude-code", "session_id": "sess-1",
                "attention": "working", "attention_at": 1.0e12, "event": "UserPromptSubmit"
            }]));
                    s
                },
                1.0e12,
            );
        assert_eq!(
            views
                .iter()
                .find(|r| r.session_id == "sess-1")
                .unwrap()
                .attention,
            "working"
        );
    }

    #[test]
    fn session_end_clears_live_rows() {
        let _g = crate::TEST_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!(
            "tokenmeter-live-end-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::env::set_var("TOKENMETER_HOME", &home);
        let live = home.join("live");
        fs::create_dir_all(&live).unwrap();
        let path = live.join("claude-code__s.json");
        fs::write(&path, r#"{"service":"claude-code","session_id":"s","attention":"working","event":"UserPromptSubmit"}"#).unwrap();
        let mut meter = Meter::new();
        meter.refresh_live();
        assert_eq!(
            meter
                .state
                .get("live")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );
        fs::remove_file(&path).unwrap();
        meter.refresh_live();
        assert_eq!(
            meter
                .state
                .get("live")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(0)
        );
    }

    #[test]
    fn overlay_snapshot_labels_check_as_확인() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let state = json!({
            "total": {"totals": {"output_tokens": 12}},
            "sessions": {},
            "live": [{"service": "claude-code", "session_id": "p",
                      "attention": "check", "attention_at": now,
                      "event": "question.asked", "project": "api"}]
        });
        let rate = LiveRate::new();
        let snap = crate::overlay::snapshot_from(&state, &rate);
        assert_eq!(snap.sessions[0].attention, "check");
        assert_eq!(attention_label(&snap.sessions[0].attention), "확인");
        assert!(crate::overlay::gauge_target(snap.rate, DEFAULT_FULL_SCALE) >= 0.0);
    }

    #[test]
    fn default_yaml_loads_known_agents() {
        let specs = watch::default_specs();
        let names: Vec<_> = specs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"claude-code"));
        assert!(names.contains(&"codex"));
        assert!(names.contains(&"opencode"));
        assert!(names.contains(&"grok"));
    }

    #[test]
    fn meter_persists_python_state_contract() {
        let path = tmp().join(format!(
            "state-contract-{}.json",
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = fs::remove_file(&path);
        let mut meter = Meter::load_test(path.clone());
        meter.ingest(TokenDelta {
            input_tokens: 10,
            cache_read: 20,
            output_tokens: 30,
            model: "claude-opus-5".into(),
            service: "claude-code".into(),
            project: "work/tokenmeter".into(),
            session: "persist-1".into(),
            vendor: "anthropic".into(),
            plan: "subscription".into(),
            endpoint: "https://api.anthropic.com".into(),
            ctx_tokens: 50_000,
            ctx_window: 200_000,
            ..TokenDelta::default()
        });
        let saved: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved
                .pointer("/total/totals/output_tokens")
                .and_then(|v| v.as_i64()),
            Some(30)
        );
        assert_eq!(
            saved
                .pointer("/today/totals/calls")
                .and_then(|v| v.as_i64()),
            Some(1)
        );
        assert_eq!(
            saved
                .pointer("/projects/work~1tokenmeter/totals/input_tokens")
                .and_then(|v| v.as_i64()),
            Some(10)
        );
        assert_eq!(
            saved
                .pointer("/sessions/claude-code~1persist-1/ctx_win")
                .and_then(|v| v.as_i64()),
            Some(200_000)
        );
        assert!(
            saved
                .pointer("/total/totals/cost_usd")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                > 0.0
        );
        let loaded = Meter::load_test(path.clone());
        assert_eq!(
            loaded
                .state
                .pointer("/total/totals/output_tokens")
                .and_then(|v| v.as_i64()),
            Some(30)
        );
        let _ = fs::remove_file(path);
    }
}
