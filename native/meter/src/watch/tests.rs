use super::probe::{probe_file, resolve_endpoint, resolve_plan};
use super::*;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;
use tokenmeter_hook::{data_dir, live_path};

#[test]
fn config_merge_and_toml_probe_match_python() {
    let mut base: serde_yaml::Value = serde_yaml::from_str(
        "settings: {poll_seconds: 2}\nservices: {codex: {enabled: true, roots: [a]}}",
    )
    .unwrap();
    let over = serde_yaml::from_str("services: {codex: {roots: [b]}}").unwrap();
    deep_merge_yaml(&mut base, over);
    assert_eq!(yaml_f64(&base, &["settings", "poll_seconds"]), Some(2.0));
    assert_eq!(yaml_at(&base, &["services", "codex", "roots", "0"]), None);
    let roots = yaml_at(&base, &["services", "codex", "roots"])
        .and_then(|value| value.as_sequence())
        .unwrap();
    assert_eq!(roots[0].as_str(), Some("b"));
    let parsed =
        specs_from_yaml("services: {custom: {match: {enabled: true, code: 7}, roots: []}}");
    assert!(
        matches!(parsed[0].match_fields["enabled"], MatchWant::One(ref value) if value == "true")
    );
    assert!(
        matches!(parsed[0].match_fields["code"], MatchWant::One(ref value) if value == "7")
    );

    let path =
        std::env::temp_dir().join(format!("tokenmeter-config-{}.toml", std::process::id()));
    fs::write(
        &path,
        "[model_providers.openai]\nbase_url = \"https://example.test/v1\"\n",
    )
    .unwrap();
    assert_eq!(
        probe_file(&path, "model_providers.openai.base_url")
            .and_then(|value| value.as_str().map(str::to_string)),
        Some("https://example.test/v1".into())
    );
    let _ = fs::remove_file(path);
}

#[test]
fn tokenmeter_home_root_uses_data_dir() {
    let _g = crate::TEST_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = std::env::temp_dir().join(format!("tokenmeter-home-{}", std::process::id()));
    std::env::set_var("TOKENMETER_HOME", &home);
    assert_eq!(expand_home("$TOKENMETER_HOME/cursor"), home.join("cursor"));
}

/// 패키지 services.yaml 의 진짜 스펙에 roots 만 갈아끼운다.
fn spec_at(name: &str, root: &Path) -> ServiceSpec {
    let mut spec = default_specs()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("services.yaml 에 {name} 가 없다"));
    spec.roots = vec![root.to_string_lossy().into_owned()];
    spec
}

fn vec4(d: &TokenDelta) -> (i64, i64, i64, i64) {
    (d.input_tokens, d.cache_read, d.cache_write, d.output_tokens)
}

fn append(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    f.write_all(text.as_bytes()).unwrap();
}

fn lines(records: &[Value]) -> String {
    records.iter().map(|r| format!("{r}\n")).collect()
}

/// json 포맷은 mtime 으로 변경을 본다 — 같은 초에 두 번 써도 놓치지 않게 못 박는다.
fn write_json(path: &Path, value: &Value, tick: u64) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value.to_string()).unwrap();
    let when = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick);
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

fn claude_record(uuid: &str) -> Value {
    json!({
        "type": "assistant", "uuid": uuid, "cwd": "/Users/dev/projects/tokenmeter",
        "sessionId": "sess-claude-1",
        "message": {"model": "claude-opus-5", "usage": {
            "input_tokens": 2, "cache_creation_input_tokens": 2161,
            "cache_read_input_tokens": 60955, "output_tokens": 813
        }}
    })
}

fn codex_record(inp: i64, cached: i64, write: i64, out: i64, last: i64, window: i64) -> Value {
    json!({"type": "event_msg", "payload": {"type": "token_count", "info": {
        "total_token_usage": {
            "input_tokens": inp, "cached_input_tokens": cached,
            "cache_write_input_tokens": write, "output_tokens": out,
            "reasoning_output_tokens": 129
        },
        "last_token_usage": {"total_tokens": last},
        "model_context_window": window
    }}})
}

fn grok_turn(event: &str, inp: i64, cached: i64, write: i64, out: i64, duration_ms: i64) -> Value {
    let mut usage = json!({
        "inputTokens": inp, "cachedReadTokens": cached, "cacheCreationTokens": write,
        "outputTokens": out, "reasoningTokens": 7308, "totalTokens": inp + out
    });
    if duration_ms > 0 {
        usage["apiDurationMs"] = json!(duration_ms);
    }
    json!({"method": "_x.ai/session/update", "params": {
        "sessionId": "sess-grok-1", "_meta": {"eventId": event},
        "update": {"sessionUpdate": "turn_completed", "prompt_id": "prompt-1", "usage": usage}
    }})
}

fn grok_chunk(event: &str, text: &str, thought: bool, total: i64) -> Value {
    json!({"method": "session/update", "params": {
        "sessionId": "sess-grok-1",
        "_meta": {"eventId": event, "promptId": "prompt-1", "totalTokens": total},
        "update": {
            "sessionUpdate": if thought { "agent_thought_chunk" } else { "agent_message_chunk" },
            "content": {"type": "text", "text": text}
        }
    }})
}

#[test]
fn committed_fixtures_regress_every_enabled_service() {
    let _home = crate::test_home("fixtures");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .canonicalize()
        .unwrap();
    let want = [
        ("claude-code", (2, 60955, 2161, 813), "claude-opus-5", "sess-claude-1", "projects/tokenmeter", "anthropic"),
        ("codex", (16023, 34304, 0, 384), "gpt-5.6-sol", "sess-codex-1", "projects/tokenmeter", "openai"),
        ("opencode", (3265, 25344, 0, 88), "nemotron-3-ultra-free", "ses_1", "Users/dev", "opencode"),
        ("grok", (122167, 1052672, 0, 12855), "grok-4.6-build", "sess-grok-1", "", "xai"),
        ("cursor", (15, 80, 5, 7), "cursor-grok-4.6", "s1", "work/token-pet", "cursor"),
    ];
    let mut enabled: Vec<String> = default_specs().into_iter().map(|s| s.name).collect();
    enabled.sort();
    let mut covered: Vec<String> = want.iter().map(|w| w.0.to_string()).collect();
    covered.sort();
    assert_eq!(enabled, covered, "켜진 서비스마다 fixture 가 있어야 한다");
    for (name, vector, model, session, project, vendor) in want {
        let deltas = ServiceReader::new(spec_at(name, &root.join(name))).poll();
        assert_eq!(deltas.len(), 1, "{name}");
        let d = &deltas[0];
        assert_eq!(vec4(d), vector, "{name}");
        assert_eq!(
            (d.service.as_str(), d.model.as_str(), d.session.as_str(), d.project.as_str(), d.vendor.as_str()),
            (name, model, session, project, vendor),
            "{name}"
        );
    }
}

#[test]
fn enabled_services_declare_token_fields() {
    for spec in default_specs() {
        let has = |f: &str| spec.fields.get(f).cloned().flatten().is_some();
        assert!(has("output") || has("input"), "{}", spec.name);
    }
}

#[test]
fn dig_walks_objects_and_array_indexes() {
    let obj = json!({"a": {"b": [{"c": 1}, {"c": 2}]}, "n": null, "zero": 0});
    assert_eq!(dig(&obj, "a.b.1.c"), Some(&json!(2)));
    assert_eq!(dig(&obj, "a.b.0.c"), Some(&json!(1)));
    assert_eq!(dig(&obj, "zero"), Some(&json!(0)));
    for missing in ["a.b.9.c", "a.b.x.c", "a.nope", "n.deeper", ""] {
        assert_eq!(dig(&obj, missing), None, "{missing}");
    }
}

#[test]
fn claude_delta_dedups_uuid_filters_type_and_skips_broken_lines() {
    let (_g, tmp) = crate::test_home("claude");
    let root = tmp.join("projects");
    let path = root.join("slug/sess-1.jsonl");
    append(&path, &lines(&[claude_record("u-1")]));
    let mut reader = ServiceReader::new(spec_at("claude-code", &root));
    let got = reader.poll();
    assert_eq!(got.len(), 1);
    assert_eq!(vec4(&got[0]), (2, 60955, 2161, 813));
    assert!(matches!(got[0].plan.as_str(), "subscription" | "api"), "{}", got[0].plan);

    append(&path, &lines(&[claude_record("u-1")]));
    assert!(reader.poll().is_empty(), "같은 uuid 를 두 번 먹으면 안 된다");
    let mut user = claude_record("u-2");
    user["type"] = json!("user");
    append(&path, &lines(&[user]));
    assert!(reader.poll().is_empty(), "match(type=assistant) 밖은 무시한다");
    append(&path, "{\"type\":\"assistant\",\"uuid\"\n");
    append(&path, &lines(&[claude_record("u-3")]));
    let got = reader.poll();
    assert_eq!(got.len(), 1, "깨진 줄은 건너뛰고 다음 줄은 먹는다");
    assert_eq!(vec4(&got[0]), (2, 60955, 2161, 813));
}

#[test]
fn prime_skips_history_and_forked_copies_are_not_recounted() {
    let (_g, tmp) = crate::test_home("prime");
    let root = tmp.join("projects");
    let old: Vec<Value> = (0..5).map(|i| claude_record(&format!("u-{i}"))).collect();
    append(&root.join("slug/a.jsonl"), &lines(&old));
    let mut reader = ServiceReader::new(spec_at("claude-code", &root));
    reader.prime();
    assert!(reader.poll().is_empty(), "기동 시 과거 로그를 먹으면 안 된다");

    let mut fresh = claude_record("u-new");
    fresh["message"]["usage"] = json!({
        "input_tokens": 7, "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0, "output_tokens": 11
    });
    let mut fork = old.clone();
    fork.push(fresh);
    append(&root.join("slug/b.jsonl"), &lines(&fork));
    let got = reader.poll();
    assert_eq!(got.len(), 1, "fork 로 복사된 레코드를 다시 먹으면 이중계상이다");
    assert_eq!(vec4(&got[0]), (7, 0, 0, 11));
}

#[test]
fn codex_cumulative_takes_increments_and_rebaselines() {
    let (_g, tmp) = crate::test_home("codex");
    let root = tmp.join("sessions");
    let path = root.join("2026/08/10/rollout-1.jsonl");
    let cwd = "/Users/dev/projects/tokenmeter";
    append(&path, &lines(&[
        json!({"type": "session_meta", "payload": {"cwd": cwd, "session_id": "sess-codex-1", "model_provider": "openai"}}),
        json!({"type": "turn_context", "payload": {"cwd": cwd, "model": "gpt-5.6-sol"}}),
        codex_record(50327, 34304, 0, 384, 0, 0),
    ]));
    let mut reader = ServiceReader::new(spec_at("codex", &root));
    let got = reader.poll();
    assert_eq!(vec4(&got[0]), (16023, 34304, 0, 384), "input 은 캐시를 포함, reasoning 은 더하지 않는다");
    assert_eq!(
        (got[0].model.as_str(), got[0].project.as_str(), got[0].session.as_str(), got[0].vendor.as_str()),
        ("gpt-5.6-sol", "projects/tokenmeter", "sess-codex-1", "openai")
    );
    append(&path, &lines(&[codex_record(60327, 40304, 100, 584, 0, 0)]));
    assert_eq!(vec4(&reader.poll()[0]), (4000, 6000, 100, 200));
    append(&path, &lines(&[codex_record(60327, 40304, 100, 584, 0, 0)]));
    assert!(reader.poll().is_empty(), "같은 누적치는 증가분 0");
    append(&path, &lines(&[codex_record(10, 0, 0, 5, 0, 0)]));
    assert!(reader.poll().is_empty(), "누적치가 줄면 baseline 만 갱신한다");
    append(&path, &lines(&[codex_record(110, 0, 0, 15, 0, 0)]));
    assert_eq!(vec4(&reader.poll()[0]), (100, 0, 0, 10));

    let other = root.join("2026/08/11/rollout-2.jsonl");
    append(&other, &lines(&[codex_record(50000, 0, 0, 1000, 0, 0)]));
    let mut primed = ServiceReader::new(spec_at("codex", &root));
    primed.prime();
    assert!(primed.poll().is_empty());
    append(&other, &lines(&[codex_record(50500, 0, 0, 1200, 0, 0)]));
    let got = primed.poll();
    assert_eq!(got.len(), 1);
    assert_eq!(vec4(&got[0]), (500, 0, 0, 200), "prime 은 현재 누적치를 baseline 으로 잡는다");
}

#[test]
fn opencode_message_file_counts_once_when_completed() {
    let (_g, tmp) = crate::test_home("opencode");
    let root = tmp.join("message");
    let path = root.join("ses_1/msg_1.json");
    let mut rec = json!({
        "id": "msg_1", "role": "assistant", "sessionID": "ses_1",
        "modelID": "nemotron-3-ultra-free", "providerID": "opencode",
        "path": {"cwd": "/Users/dev"}, "cost": 0,
        "tokens": {"total": 0, "input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}}
    });
    write_json(&path, &rec, 1);
    let mut reader = ServiceReader::new(spec_at("opencode", &root));
    assert!(reader.poll().is_empty(), "생성 시점(토큰 0)에는 먹지 않는다");
    rec["tokens"] = json!({"total": 28697, "input": 3265, "output": 88, "reasoning": 68,
                           "cache": {"read": 25344, "write": 0}});
    write_json(&path, &rec, 2);
    let got = reader.poll();
    assert_eq!(got.len(), 1);
    assert_eq!(vec4(&got[0]), (3265, 25344, 0, 88));
    assert_eq!(
        (got[0].model.as_str(), got[0].project.as_str(), got[0].session.as_str(),
         got[0].vendor.as_str(), got[0].plan.as_str()),
        ("nemotron-3-ultra-free", "Users/dev", "ses_1", "opencode", "api"),
        "벤더는 모델명이 아니라 게이트웨이(providerID)다"
    );
    write_json(&path, &rec, 3);
    assert!(reader.poll().is_empty(), "같은 내용이 다시 쓰여도 증가분 0");
}

#[test]
fn grok_turns_dedup_and_streaming_chunks_estimate_output() {
    let (_g, tmp) = crate::test_home("grok");
    let root = tmp.join("sessions");
    let path = root.join("%2FUsers%2Fdev/sess-grok-1/updates.jsonl");
    append(&path, &lines(&[
        json!({"method": "_x.ai/session/update", "params": {"sessionId": "sess-grok-1",
               "_meta": {"totalTokens": 100986}, "update": {"sessionUpdate": "agent_message_chunk"}}}),
        grok_turn("e-1", 1174839, 1052672, 0, 12855, 0),
    ]));
    let mut reader = ServiceReader::new(spec_at("grok", &root));
    let got = reader.poll();
    assert_eq!(got.len(), 1, "본문 없는 스트리밍 줄은 먹지 않는다");
    assert_eq!(vec4(&got[0]), (122167, 1052672, 0, 12855));
    assert_eq!((got[0].model.as_str(), got[0].vendor.as_str(), got[0].session.as_str()),
               ("grok-4.6-build", "xai", "sess-grok-1"));
    assert_eq!(got[0].ctx_tokens, 0, "turn_completed 입력 합계로 ctx% 를 지어내면 안 된다");
    append(&path, &lines(&[grok_turn("e-1", 1174839, 1052672, 0, 12855, 0)]));
    assert!(reader.poll().is_empty(), "같은 eventId 는 한 번만");
    append(&path, &lines(&[grok_turn("e-2", 20000, 15000, 100, 300, 0)]));
    assert_eq!(vec4(&reader.poll()[0]), (5000, 15000, 100, 300));

    let stream = tmp.join("stream");
    let path = stream.join("sess/updates.jsonl");
    append(&path, &lines(&[grok_chunk("c-1", &"abcd".repeat(3), true, 1000)]));
    let mut reader = ServiceReader::new(spec_at("grok", &stream));
    let got = reader.poll();
    assert_eq!((got[0].output_tokens, got[0].ctx_tokens, got[0].duration_ms), (3, 1000, 0));
    append(&path, &lines(&[grok_chunk("c-2", "xyz", false, 1100)]));
    let got = reader.poll();
    assert_eq!((got[0].output_tokens, got[0].ctx_tokens), (1, 1100));
    append(&path, &lines(&[grok_turn("e-1", 100, 0, 0, 40, 8000)]));
    let got = reader.poll();
    assert_eq!(got[0].output_tokens, 36, "턴이 끝나면 공식 출력과의 차(40 - 3 - 1)만 더한다");
    assert_eq!(got[0].duration_ms, 8000);
}

#[test]
fn context_tokens_follow_each_service_spec() {
    let (_g, tmp) = crate::test_home("ctx");
    let root = tmp.join("projects");
    let mut reader = ServiceReader::new(spec_at("claude-code", &root));
    append(&root.join("slug/s.jsonl"), &lines(&[claude_record("u-1")]));
    let got = reader.poll();
    assert_eq!(got[0].ctx_tokens, 2 + 60955 + 2161, "출력은 컨텍스트가 아니다");
    assert_eq!(got[0].ctx_window, 1_000_000, "가격표의 window 가 분모다");

    let mut big = claude_record("u-big");
    big["message"]["usage"] = json!({"input_tokens": 2, "cache_read_input_tokens": 430_000,
                                     "cache_creation_input_tokens": 5_000, "output_tokens": 100});
    append(&root.join("slug/big.jsonl"), &lines(&[big.clone()]));
    let got = reader.poll();
    assert!(got[0].ctx_tokens > 200_000 && got[0].ctx_window == 1_000_000);

    big["uuid"] = json!("u-sub");
    big["isSidechain"] = json!(true);
    append(&root.join("slug/sub.jsonl"), &lines(&[big]));
    let got = reader.poll();
    assert!(got[0].total() > 0 && got[0].subagent, "서브에이전트 토큰은 합산한다");
    assert_eq!(got[0].ctx_tokens, 0, "남의 컨텍스트가 세션 ctx% 를 덮으면 안 된다");

    let croot = tmp.join("sessions");
    let mut creader = ServiceReader::new(spec_at("codex", &croot));
    append(&croot.join("r.jsonl"), &lines(&[
        json!({"type": "turn_context", "payload": {"cwd": "/a/tokenmeter", "model": "gpt-5.6-sol"}}),
        codex_record(4_657_501, 4_343_296, 0, 20_577, 123_165, 353_400),
    ]));
    let got = creader.poll();
    assert_eq!(got[0].ctx_tokens, 123_165, "세션 누적이 컨텍스트로 잡히면 늘 100% 다");
    assert_eq!(got[0].ctx_window, 353_400, "로그가 알려주는 창이 가격표보다 우선한다");
    append(&croot.join("r.jsonl"), &lines(&[codex_record(4_700_000, 4_343_296, 0, 20_800, 12_000, 353_400)]));
    assert_eq!(creader.poll()[0].ctx_tokens, 12_000, "압축되면 그대로 내려간다");
}

#[test]
fn plan_and_endpoint_probes_follow_env_files_and_live_routing() {
    let (_g, tmp) = crate::test_home("probe");
    for var in ["TP_FAKE_KEY", "ANTHROPIC_BASE_URL", "CLAUDE_CODE_USE_BEDROCK", "CLAUDE_CODE_USE_VERTEX"] {
        std::env::remove_var(var);
    }
    let yaml = |text: String| serde_yaml::from_str::<serde_yaml::Value>(&text).unwrap();
    let env_probe = yaml("{env: [TP_FAKE_KEY], if_set: api, else: subscription}".into());
    let explicit = ServiceSpec { plan: "subscription".into(), plan_probe: env_probe.clone(), ..Default::default() };
    assert_eq!(resolve_plan(&explicit), "subscription", "명시값이 항상 이긴다");
    let by_env = ServiceSpec { plan_probe: env_probe, ..Default::default() };
    assert_eq!(resolve_plan(&by_env), "subscription");
    std::env::set_var("TP_FAKE_KEY", "sk-1");
    assert_eq!(resolve_plan(&by_env), "api");
    std::env::remove_var("TP_FAKE_KEY");

    let auth = tmp.join("auth.json");
    let by_file = ServiceSpec {
        plan_probe: yaml(format!(
            "{{path: {}, key: auth_mode, map: {{chatgpt: subscription, apikey: api}}, default: unknown}}",
            json!(auth.display().to_string())
        )),
        ..Default::default()
    };
    fs::write(&auth, r#"{"auth_mode": "chatgpt"}"#).unwrap();
    assert_eq!(resolve_plan(&by_file), "subscription");
    fs::write(&auth, r#"{"auth_mode": "apikey"}"#).unwrap();
    assert_eq!(resolve_plan(&by_file), "api");
    fs::write(&auth, "깨진 파일").unwrap();
    assert_eq!(resolve_plan(&by_file), "unknown", "프로브가 실패해도 죽으면 안 된다");
    fs::remove_file(&auth).unwrap();
    assert_eq!(resolve_plan(&by_file), "unknown");
    assert_eq!(resolve_plan(&ServiceSpec::default()), "unknown");

    for (model, vendor) in [("claude-opus-5", "anthropic"), ("gpt-5.6-sol", "openai"),
                            ("nemotron-3-ultra-free", "nvidia"), ("무슨-모델", "unknown"), ("", "unknown")] {
        assert_eq!(crate::pricing::vendor_of(model), vendor, "{model}");
    }

    let claude = spec_at("claude-code", &tmp);
    let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    };
    assert_eq!(resolve_endpoint(&claude, Some(&env(&[])), "anthropic", "api"), "https://api.anthropic.com");
    assert_eq!(
        resolve_endpoint(&claude, Some(&env(&[("ANTHROPIC_BASE_URL", "https://llm.mycorp.com/v1")])), "anthropic", "api"),
        "https://llm.mycorp.com/v1"
    );
    assert_eq!(resolve_endpoint(&claude, Some(&env(&[("CLAUDE_CODE_USE_BEDROCK", "1")])), "anthropic", "api"), "bedrock");

    fs::create_dir_all(tokenmeter_hook::live_dir()).unwrap();
    fs::write(live_path("claude-code", "s-bedrock"), r#"{"routing_env": {"CLAUDE_CODE_USE_BEDROCK": "1"}}"#).unwrap();
    let mut reader = ServiceReader::new(claude);
    assert_eq!(reader.endpoint_for("s-bedrock", "anthropic"), "bedrock", "훅이 찍은 세션 환경을 읽는다");
    assert_eq!(reader.endpoint_for("s-없음", "anthropic"), "https://api.anthropic.com");
}

#[test]
fn toggle_file_switches_measurement_services_and_overlay() {
    let (_g, _tmp) = crate::test_home("toggle");
    let toggle = data_dir().join("toggle.json");
    let names = || -> Vec<String> { load_runtime_config().specs.into_iter().map(|s| s.name).collect() };
    let settings = || load_runtime_config().settings;
    assert!(settings().enabled && settings().overlay_auto, "파일이 없으면 켜진 것으로 본다");
    fs::write(&toggle, r#"{"enabled": false}"#).unwrap();
    assert!(!settings().enabled, "전체를 끄면 데몬이 뜨지 않는다");
    fs::write(&toggle, r#"{"services": {"codex": false}}"#).unwrap();
    assert!(settings().enabled);
    assert!(!names().contains(&"codex".to_string()) && names().contains(&"claude-code".to_string()));
    fs::write(&toggle, r#"{"overlay": false}"#).unwrap();
    assert!(!settings().overlay_auto && settings().enabled, "창을 끈 것이 측정을 끄면 안 된다");
    fs::write(&toggle, "{망가짐").unwrap();
    assert!(settings().enabled, "깨진 파일 하나 때문에 측정이 멈추면 안 된다");
}

fn write_user_services(root: &Path, text: &str) {
    let dir = root.join("config/tokenmeter");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("services.yaml"), text).unwrap();
}

fn loaded_names() -> Vec<String> {
    load_all_specs().into_iter().map(|s| s.name).collect()
}

#[test]
fn adapters_dir_is_the_builtin_list() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("adapters");
    let mut files: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok()?.strip_suffix(".yaml").map(str::to_string))
        .collect();
    files.sort();
    let ids: Vec<&str> = ADAPTERS.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, files);
    let (_g, _t) = crate::test_home("adapters-list");
    let mut loaded = loaded_names();
    loaded.sort();
    assert_eq!(loaded, files);
}

#[test]
fn a_bad_user_block_drops_only_that_service() {
    let (_g, root) = crate::test_home("bad-block");
    write_user_services(
        &root,
        "services:\n  codex:\n    roots: \"not-a-list\"\n  mine:\n    roots: [\"~/x\"]\n    format: jsonl\n    fields: {output: n}\n    colour: red\n",
    );
    let names = loaded_names();
    assert!(!names.contains(&"codex".to_string()));
    assert!(names.contains(&"claude-code".to_string()) && names.contains(&"mine".to_string()));
    let report = load_report();
    assert!(report.skipped.iter().any(|(id, why)| id == "codex" && why.contains("roots")), "{:?}", report.skipped);
    assert_eq!(report.warnings, vec!["mine: unknown key colour".to_string()], "사용자 블록의 모르는 키는 경고뿐");
}

#[test]
fn unknown_format_or_mode_is_a_validation_error() {
    let (_g, root) = crate::test_home("bad-enum");
    write_user_services(
        &root,
        "services:\n  a:\n    roots: [\"~/a\"]\n    format: jsnl\n    fields: {output: n}\n  b:\n    roots: [\"~/b\"]\n    mode: cumulativ\n    fields: {output: n}\n",
    );
    let names = loaded_names();
    assert!(!names.contains(&"a".to_string()) && !names.contains(&"b".to_string()));
    assert!(names.contains(&"codex".to_string()));
    let skipped = load_report().skipped;
    assert!(skipped.iter().any(|(id, why)| id == "a" && why.contains("format")), "{skipped:?}");
    assert!(skipped.iter().any(|(id, why)| id == "b" && why.contains("mode")), "{skipped:?}");
}

#[test]
fn disabled_builtin_is_still_builtin() {
    let (_g, root) = crate::test_home("disabled-builtin");
    write_user_services(&root, "services:\n  cursor:\n    enabled: false\n  mine:\n    roots: [\"~/x\"]\n    fields: {output: n}\n");
    assert!(!load_runtime_config().specs.iter().any(|s| s.name == "cursor"));
    assert!(is_builtin_service("cursor"), "꺼 둔 기본 서비스도 리그에서 제 이름으로 간다");
    assert!(!is_builtin_service("mine"));
}

#[test]
fn builtin_adapters_have_no_unknown_keys() {
    let (_g, _t) = crate::test_home("unknown-keys");
    let report = load_report();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.skipped.is_empty(), "{:?}", report.skipped);
}
