use super::expr::{Env, Pick};
use super::ledger;
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
    let conds = Compiled::new(&parsed[0]).unwrap().conds;
    let rec = json!({"enabled": true, "code": 7});
    assert!(cond::all(&conds, &Env::new(&rec, &|_| None)), "YAML 불리언·숫자는 글자로 비교");

    let path =
        std::env::temp_dir().join(format!("tokenmeter-config-{}.toml", std::process::id()));
    fs::write(
        &path,
        "[model_providers.openai]\nbase_url = \"https://example.test/v1\"\n",
    )
    .unwrap();
    let key = Pick::from_yaml(&"model_providers[$ctx.vendor].base_url".into()).unwrap();
    assert_eq!(
        probe_file(&path, &key, "openai")
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
    let root = vec![root.to_string_lossy().into_owned()];
    for s in &mut spec.sources {
        s.roots = root.clone();
    }
    spec.roots = root;
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
    let usage = json!({
        "input_tokens": inp, "cached_input_tokens": cached,
        "cache_write_input_tokens": write, "output_tokens": out,
        "reasoning_output_tokens": 129
    });
    let mut last_usage = usage.clone();
    last_usage["total_tokens"] = json!(last);
    json!({"type": "event_msg", "payload": {"type": "token_count", "info": {
        "total_token_usage": usage,
        "last_token_usage": last_usage,
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
fn enabled_services_declare_token_fields() {
    for spec in default_specs() {
        let views = if spec.sources.is_empty() { vec![&spec] } else { spec.sources.iter().collect() };
        for view in views {
            let has = |f: &str| view.fields.get(f).is_some_and(|v| !v.is_null());
            assert!(has("output") || has("input") || !view.cost_usd.is_null(), "{}", spec.name);
        }
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

/// 인라인 서비스 `t` 하나(로더 검증 없이). `body`는 흐름 매핑 안의 항목들.
fn inline(root: &Path, body: &str) -> ServiceReader {
    let text = format!("services: {{t: {{roots: [{root:?}], {body}}}}}");
    let spec = specs_from_yaml(&text).pop().unwrap_or_else(|| panic!("인라인 스펙: {text}"));
    ServiceReader::new(spec)
}

fn sum4(got: &[TokenDelta]) -> (i64, i64, i64, i64) {
    got.iter().fold((0, 0, 0, 0), |a, d| {
        (a.0 + d.input_tokens, a.1 + d.cache_read, a.2 + d.cache_write, a.3 + d.output_tokens)
    })
}

fn calls(got: &[TokenDelta]) -> u32 {
    got.iter().map(|d| d.calls).sum()
}

const CLAUDE_KEYED: &str = r#"match: {type: assistant}, key: "message.id & requestId",
    fields: {input: message.usage.input_tokens, cache_read: message.usage.cache_read_input_tokens,
             cache_write: message.usage.cache_creation_input_tokens, output: message.usage.output_tokens},
    context: {session: sessionId}"#;

/// Claude 블록 줄: 같은 호출이면 `message.id`·`requestId`·usage가 같고 output만 는다(스펙 3.1).
fn block_line(uuid: &str, id: &str, req: &str, out: i64) -> Value {
    json!({"type": "assistant", "uuid": uuid, "requestId": req, "sessionId": "s-1",
           "message": {"id": id, "usage": {"input_tokens": 2, "cache_read_input_tokens": 100,
                                           "cache_creation_input_tokens": 10, "output_tokens": out}}})
}

#[test]
fn delta_key_keeps_max_across_block_lines() {
    let (_g, tmp) = crate::test_home("max-keep");
    let root = tmp.join("projects");
    let path = root.join("slug/s.jsonl");
    let mut user = block_line("u-9", "m-9", "r-9", 500);
    user["type"] = json!("user");
    append(&path, &lines(&[
        block_line("u-1", "m-1", "r-1", 31),
        block_line("u-2", "m-1", "r-1", 31),
        user,
        block_line("u-3", "m-1", "r-1", 300),
    ]));
    append(&path, "{\"type\":\"assistant\",\"uuid\"\n");
    append(&path, &lines(&[block_line("u-4", "m-2", "r-2", 7)]));
    let mut reader = inline(&root, CLAUDE_KEYED);
    let got = reader.poll();
    // 델타는 31, +269, 둘째 호출. 호출 수는 키가 처음 0 아닌 값을 낼 때만 센다.
    let outs: Vec<i64> = got.iter().map(|d| d.output_tokens).collect();
    assert_eq!(outs, [31, 269, 7], "user 줄은 match 밖, 깨진 줄은 건너뛴다");
    assert_eq!(sum4(&got), (2 + 2, 100 + 100, 10 + 10, 300 + 7));
    assert_eq!(calls(&got), 2);
    append(&path, &lines(&[block_line("u-5", "m-1", "r-1", 300), block_line("u-6", "m-2", "r-2", 3)]));
    assert!(reader.poll().is_empty(), "같거나 작은 복사본은 늘어난 것이 없다");
}

#[test]
fn forked_copy_with_same_key_is_not_recounted() {
    let (_g, tmp) = crate::test_home("fork");
    let root = tmp.join("projects");
    let old: Vec<Value> = (0..5).map(|i| block_line(&format!("u-{i}"), &format!("m-{i}"), "r", 40)).collect();
    append(&root.join("slug/a.jsonl"), &lines(&old));
    let mut reader = inline(&root, CLAUDE_KEYED);
    reader.prime();
    assert!(reader.poll().is_empty(), "기동 때 과거 로그를 내지 않는다");
    let mut fork = old.clone();
    fork.push(block_line("u-new", "m-new", "r", 11));
    append(&root.join("slug/b.jsonl"), &lines(&fork));
    let got = reader.poll();
    assert_eq!(got.len(), 1, "포크로 복사된 레코드를 다시 세면 이중 계상이다");
    assert_eq!((vec4(&got[0]), got[0].calls), ((2, 100, 10, 11), 1));
}

#[test]
fn b1_zero_first_copy_then_real_value_counts() {
    let (_g, tmp) = crate::test_home("b1");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    let mut reader = inline(&root, "key: id, fields: {output: out}");
    append(&path, &lines(&[json!({"id": "a", "out": 0})]));
    assert!(reader.poll().is_empty(), "0인 첫 복사본은 내지 않는다");
    append(&path, &lines(&[json!({"id": "a", "out": 50}), json!({"id": "b", "out": 0}), json!({"id": "b", "out": 40})]));
    let got = reader.poll();
    assert_eq!(got.iter().map(|d| d.output_tokens).collect::<Vec<_>>(), [50, 40], "0이 키를 태우지 않는다");
    assert_eq!(calls(&got), 2);
}

#[test]
fn b3_recent_keys_survive_many_old_keys() {
    let (_g, tmp) = crate::test_home("b3");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    let mut reader = inline(&root, "key: id, fields: {output: out}");
    reader.ledger = ledger::Ledger::new(now_secs, 3);
    let rec = |id: &str| json!({"id": id, "out": 10});
    append(&path, &lines(&["k0", "k1", "k2", "k3", "k4"].map(rec)));
    assert_eq!(reader.poll().len(), 5);
    append(&path, &lines(&[rec("k1")]));
    assert!(reader.poll().is_empty());
    reader.ledger.prune(); // 커밋 때만 버린다
    append(&path, &lines(&["k1", "k3", "k4"].map(rec)));
    assert!(reader.poll().is_empty(), "최근에 본 키 셋은 남는다");
    append(&path, &lines(&[rec("k0")]));
    assert_eq!(reader.poll().len(), 1, "오래 안 본 키부터 버린다");
}

#[test]
fn keyless_delta_copy_in_new_file_counts_once() {
    let (_g, tmp) = crate::test_home("keyless");
    let root = tmp.join("d");
    let mut reader = inline(&root, "fields: {output: out}");
    let recs = [json!({"n": 1, "out": 5}), json!({"n": 2, "out": 6})];
    append(&root.join("a.jsonl"), &lines(&recs));
    let got = reader.poll();
    assert_eq!((sum4(&got).3, calls(&got)), (11, 2));
    let mut copy = recs.to_vec();
    copy.push(json!({"out": 7, "n": 3}));
    append(&root.join("b.jsonl"), &lines(&copy));
    let got = reader.poll();
    assert_eq!((sum4(&got).3, calls(&got)), (7, 1), "레코드 해시가 키라 새 파일의 복사본은 다시 세지 않는다");
}

#[test]
fn cumulative_stream_in_second_file_uses_first_baseline() {
    let (_g, tmp) = crate::test_home("cum-copy");
    let root = tmp.join("d");
    let mut reader = inline(&root, "mode: cumulative, key: sid, fields: {output: out}");
    append(&root.join("a.jsonl"), &lines(&[json!({"sid": "s1", "out": 100})]));
    let got = reader.poll();
    assert_eq!((sum4(&got).3, calls(&got)), (100, 1), "처음 본 스트림은 지금처럼 전체");
    append(&root.join("b.jsonl"), &lines(&[
        json!({"sid": "s1", "out": 100}),
        json!({"sid": "s1", "out": 130}),
        json!({"sid": "s2", "out": 9}),
    ]));
    let got = reader.poll();
    assert_eq!(got.iter().map(|d| d.output_tokens).collect::<Vec<_>>(), [30, 9], "s1은 a.jsonl의 기준값에서 잇는다");
    assert_eq!(calls(&got), 1, "s1은 이미 호출로 셌다");
}

#[test]
fn rebase_on_change_resets_baseline_only() {
    let (_g, tmp) = crate::test_home("rebase");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    let mut reader = inline(&root, "mode: cumulative, key: sid, rebase_on: run, fields: {output: out}, context: {model: model}");
    append(&path, &lines(&[json!({"run": "r1", "sid": "s", "out": 10, "model": "m1"})]));
    assert_eq!(sum4(&reader.poll()).3, 10);
    append(&path, &lines(&[json!({"run": "r1", "sid": "s", "out": 25})]));
    assert_eq!(sum4(&reader.poll()).3, 15);
    append(&path, &lines(&[
        json!({"run": "r2", "sid": "s", "out": 4, "model": "m2"}),
        json!({"run": "r2", "sid": "s", "out": 6}),
    ]));
    assert!(reader.poll().is_empty(), "바뀐 읽기에서는 기준값만 잡는다(겹친 +2는 잃는다)");
    append(&path, &lines(&[json!({"run": "r2", "sid": "s", "out": 9})]));
    let got = reader.poll();
    assert_eq!((got.len(), got[0].output_tokens, got[0].model.as_str()), (1, 3, "m2"), "문맥은 그대로 배운다");
}

#[test]
fn input_includes_subtracts_only_when_parts_fit() {
    let (_g, tmp) = crate::test_home("includes");
    let root = tmp.join("d");
    let mut reader = inline(
        &root,
        "key: id, input_includes: [cache_read, cache_write], fields: {input: i, cache_read: cr, cache_write: cw, output: o}",
    );
    append(&root.join("s.jsonl"), &lines(&[
        json!({"id": 1, "i": 100, "cr": 30, "cw": 20, "o": 1}),
        json!({"id": 2, "i": 10, "cr": 30, "cw": 20, "o": 1}),
        json!({"id": 3, "i": 50, "cr": 30, "cw": 20, "o": 1}),
    ]));
    let got: Vec<_> = reader.poll().iter().map(vec4).collect();
    assert_eq!(got, [(50, 30, 20, 1), (10, 30, 20, 1), (0, 30, 20, 1)], "합이 input보다 크면(섞인 모양) 빼지 않는다");
}

#[test]
fn field_final_value_is_not_negative() {
    let (_g, tmp) = crate::test_home("negative");
    let root = tmp.join("d");
    let mut reader = inline(&root, "key: id, fields: {input: n, output: \"a - b\"}");
    append(&root.join("s.jsonl"), &lines(&[
        json!({"id": 1, "n": 5, "a": 3, "b": 10}),
        json!({"id": 2, "n": 0, "a": 10, "b": 3}),
    ]));
    let got: Vec<_> = reader.poll().iter().map(vec4).collect();
    assert_eq!(got, [(5, 0, 0, 0), (0, 0, 0, 7)]);
}

#[test]
fn logged_cost_goes_through_the_ledger() {
    let (_g, tmp) = crate::test_home("cost");
    let root = tmp.join("d");
    let mut reader = inline(&root, "key: id, cost_usd: cost, fields: {output: out}");
    let rec = lines(&[json!({"id": "x", "out": 5, "cost": 0.5})]);
    append(&root.join("a.jsonl"), &rec);
    let got = reader.poll();
    assert_eq!((got[0].output_tokens, got[0].cost_usd, got[0].calls), (5, Some(0.5), 1));
    append(&root.join("a.jsonl"), &rec);
    append(&root.join("b.jsonl"), &rec);
    assert!(reader.poll().is_empty(), "같은 키를 두 번 읽어도 비용은 한 번");
    append(&root.join("a.jsonl"), &lines(&[json!({"id": "x", "out": 5, "cost": 0.75})]));
    let got = reader.poll();
    assert_eq!((got.len(), got[0].output_tokens, got[0].cost_usd, got[0].calls), (1, 0, Some(0.25), 0));
}

#[test]
fn cumulative_cost_and_duration_diff() {
    let (_g, tmp) = crate::test_home("cum-cost");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    let mut reader = inline(
        &root,
        "mode: cumulative, key: sid, cost_usd: total_cost, duration_ms: api_ms, fields: {output: out}",
    );
    append(&path, &lines(&[json!({"sid": "s", "out": 10, "total_cost": 0.25, "api_ms": 1000})]));
    let got = reader.poll();
    assert_eq!((got[0].output_tokens, got[0].cost_usd, got[0].duration_ms, got[0].calls), (10, Some(0.25), 1000, 1));
    append(&path, &lines(&[json!({"sid": "s", "out": 10, "total_cost": 0.75, "api_ms": 1500})]));
    let got = reader.poll();
    assert_eq!((got.len(), got[0].output_tokens, got[0].cost_usd, got[0].duration_ms), (1, 0, Some(0.5), 500));
    append(&path, &lines(&[json!({"sid": "s", "out": 12, "total_cost": 0.75, "api_ms": 1800})]));
    let got = reader.poll();
    assert_eq!((got[0].output_tokens, got[0].cost_usd, got[0].duration_ms, got[0].calls), (2, None, 300, 0));
}

#[test]
fn drop_only_when_tokens_and_cost_are_zero() {
    let (_g, tmp) = crate::test_home("drop");
    let root = tmp.join("d");
    let mut reader = inline(&root, "key: id, cost_usd: cost, duration_ms: ms, fields: {output: out}");
    append(&root.join("s.jsonl"), &lines(&[
        json!({"id": "a", "out": 0, "cost": 0.3}),
        json!({"id": "b", "out": 0, "cost": 0, "ms": 500}),
        json!({"id": "c", "out": 0}),
    ]));
    let got = reader.poll();
    assert_eq!(got.len(), 1, "시간만 있거나 모두 0이면 버린다");
    assert_eq!((got[0].total(), got[0].cost_usd, got[0].calls), (0, Some(0.3), 1), "토큰 0, 비용 0.3은 남는다(crush)");
}

#[test]
fn json_without_key_is_rejected() {
    let (_g, root) = crate::test_home("json-key");
    write_user_services(
        &root,
        "services:\n  j:\n    roots: [\"~/x\"]\n    format: json\n    fields: {output: n}\n  k:\n    roots: [\"~/x\"]\n    format: json\n    key: id\n    fields: {output: n}\n",
    );
    let names = loaded_names();
    assert!(!names.contains(&"j".to_string()) && names.contains(&"k".to_string()));
    let skipped = load_report().skipped;
    assert!(skipped.iter().any(|(id, why)| id == "j" && why.starts_with("key")), "{skipped:?}");
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
    append(&croot.join("rollout-r.jsonl"), &lines(&[
        json!({"type": "turn_context", "payload": {"cwd": "/a/tokenmeter", "model": "gpt-5.6-sol"}}),
        codex_record(4_657_501, 4_343_296, 0, 20_577, 123_165, 353_400),
    ]));
    let got = creader.poll();
    assert_eq!(got[0].ctx_tokens, 123_165, "세션 누적이 컨텍스트로 잡히면 늘 100% 다");
    assert_eq!(got[0].ctx_window, 353_400, "로그가 알려주는 창이 가격표보다 우선한다");
    append(&croot.join("rollout-r.jsonl"), &lines(&[codex_record(4_700_000, 4_343_296, 0, 20_800, 12_000, 353_400)]));
    assert_eq!(creader.poll()[0].ctx_tokens, 12_000, "압축되면 그대로 내려간다");
}

/// 로더처럼 파싱한 프로브 키로 부른다.
fn plan_of(spec: &ServiceSpec) -> String {
    resolve_plan(spec, Compiled::new(spec).unwrap().plan_key.as_ref(), "")
}

fn endpoint_of(spec: &ServiceSpec, env: Option<&HashMap<String, String>>, vendor: &str, plan: &str) -> String {
    resolve_endpoint(spec, Compiled::new(spec).unwrap().endpoint_key.as_ref(), env, vendor, plan)
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
    assert_eq!(plan_of(&explicit), "subscription", "명시값이 항상 이긴다");
    let by_env = ServiceSpec { plan_probe: env_probe, ..Default::default() };
    assert_eq!(plan_of(&by_env), "subscription");
    std::env::set_var("TP_FAKE_KEY", "sk-1");
    assert_eq!(plan_of(&by_env), "api");
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
    assert_eq!(plan_of(&by_file), "subscription");
    fs::write(&auth, r#"{"auth_mode": "apikey"}"#).unwrap();
    assert_eq!(plan_of(&by_file), "api");
    fs::write(&auth, "깨진 파일").unwrap();
    assert_eq!(plan_of(&by_file), "unknown", "프로브가 실패해도 죽으면 안 된다");
    fs::remove_file(&auth).unwrap();
    assert_eq!(plan_of(&by_file), "unknown");
    assert_eq!(plan_of(&ServiceSpec::default()), "unknown");

    for (model, vendor) in [("claude-opus-5", "anthropic"), ("gpt-5.6-sol", "openai"),
                            ("nemotron-3-ultra-free", "nvidia"), ("무슨-모델", "unknown"), ("", "unknown")] {
        assert_eq!(crate::pricing::vendor_of(model), vendor, "{model}");
    }

    let claude = spec_at("claude-code", &tmp);
    let env = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    };
    assert_eq!(endpoint_of(&claude, Some(&env(&[])), "anthropic", "api"), "https://api.anthropic.com");
    assert_eq!(
        endpoint_of(&claude, Some(&env(&[("ANTHROPIC_BASE_URL", "https://llm.mycorp.com/v1")])), "anthropic", "api"),
        "https://llm.mycorp.com/v1"
    );
    assert_eq!(endpoint_of(&claude, Some(&env(&[("CLAUDE_CODE_USE_BEDROCK", "1")])), "anthropic", "api"), "bedrock");

    fs::create_dir_all(tokenmeter_hook::live_dir()).unwrap();
    fs::write(live_path("claude-code", "s-bedrock"), r#"{"routing_env": {"CLAUDE_CODE_USE_BEDROCK": "1"}}"#).unwrap();
    let mut reader = ServiceReader::new(claude);
    assert_eq!(reader.endpoint_for("s-bedrock", "anthropic"), "bedrock", "훅이 찍은 세션 환경을 읽는다");
    assert_eq!(reader.endpoint_for("s-없음", "anthropic"), "https://api.anthropic.com");
}

fn envs(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn probe_spec(endpoint_probe: &str) -> ServiceSpec {
    ServiceSpec { endpoint_probe: serde_yaml::from_str(endpoint_probe).unwrap(), ..Default::default() }
}

#[test]
fn plan_local_is_fixed() {
    let (_g, tmp) = crate::test_home("plan-local");
    std::env::set_var("TP_LOCAL_KEY", "1");
    let mut reader = inline(
        &tmp,
        "plan: local, plan_probe: {env: [TP_LOCAL_KEY], if_set: api, else: subscription}, fields: {output: n}, context: {vendor: v}",
    );
    append(&tmp.join("a.jsonl"), &lines(&[json!({"n": 5, "v": "anthropic"})]));
    append(&tmp.join("b.jsonl"), &lines(&[json!({"n": 3, "v": "openai"})]));
    let plans: Vec<String> = reader.poll().into_iter().map(|d| d.plan).collect();
    std::env::remove_var("TP_LOCAL_KEY");
    assert_eq!(plans, ["local", "local"], "고정 요금제는 프로브보다 먼저, 벤더와 상관없이");
}

#[test]
fn plan_probe_is_resolved_per_vendor() {
    let (_g, tmp) = crate::test_home("plan-vendor");
    let auth = tmp.join("auth.json");
    fs::write(&auth, r#"{"anthropic": {"type": "oauth", "access": "비밀"}, "openai": {"type": "api", "key": "sk-x"}}"#).unwrap();
    let data = tmp.join("d");
    let mut reader = inline(
        &data,
        &format!(
            r#"plan_probe: {{path: {auth:?}, key: "[$ctx.vendor].type", map: {{oauth: subscription, api: api}}, default: unknown}},
               fields: {{output: n}}, context: {{vendor: v}}"#
        ),
    );
    for (file, vendor) in [("a", "anthropic"), ("b", "openai"), ("c", "google")] {
        append(&data.join(format!("{file}.jsonl")), &lines(&[json!({"n": 1, "v": vendor})]));
    }
    let mut got: Vec<(String, String)> = reader.poll().into_iter().map(|d| (d.vendor, d.plan)).collect();
    got.sort();
    let want = [("anthropic", "subscription"), ("google", "unknown"), ("openai", "api")];
    assert_eq!(got, want.map(|(v, p)| (v.to_string(), p.to_string())), "OpenCode auth.json은 벤더마다 다르다");
}

#[test]
fn record_endpoint_wins_and_is_normalized() {
    let (_g, tmp) = crate::test_home("record-endpoint");
    std::env::set_var("TP_REC_FLAG", "1");
    let mut reader = inline(
        &tmp,
        "endpoint_probe: {flags: {TP_REC_FLAG: flagged}}, fields: {output: n}, context: {endpoint: ep}",
    );
    append(&tmp.join("a.jsonl"), &lines(&[json!({"n": 1, "ep": "https://u:p@GW.x/v1/?k=1#f"})]));
    append(&tmp.join("b.jsonl"), &lines(&[json!({"n": 2})]));
    let mut got: Vec<(i64, String)> = reader.poll().into_iter().map(|d| (d.output_tokens, d.endpoint)).collect();
    std::env::remove_var("TP_REC_FLAG");
    got.sort();
    assert_eq!(got, [(1, "https://gw.x/v1".to_string()), (2, "flagged".to_string())], "레코드가 플래그보다 먼저, 비밀은 지운다");
}

#[test]
fn flag_probe_prefers_its_base_url() {
    let (_g, _tmp) = crate::test_home("flag-probe");
    let spec = probe_spec(
        "{flags: {TP_USE_X: {env: [TP_X_BASE_URL], default: amazon-bedrock}, TP_USE_Y: google-vertex}, default: https://api.test}",
    );
    let at = |pairs: &[(&str, &str)]| endpoint_of(&spec, Some(&envs(pairs)), "anthropic", "api");
    assert_eq!(at(&[("TP_USE_X", "1")]), "amazon-bedrock", "플래그만 있으면 그 id");
    assert_eq!(
        at(&[("TP_USE_X", "1"), ("TP_X_BASE_URL", "https://u:p@br.corp/v1?x=1")]),
        "https://br.corp/v1",
        "플래그와 base URL이 함께면 URL(정규화)"
    );
    assert_eq!(at(&[("TP_USE_X", "0"), ("TP_USE_Y", "true")]), "google-vertex", "0은 꺼진 플래그");
    assert_eq!(at(&[]), "https://api.test");
}

#[test]
fn endpoint_order() {
    let (_g, tmp) = crate::test_home("endpoint-order");
    for var in ["TP_FLAG", "TP_URL_A", "TP_URL_B"] {
        std::env::remove_var(var);
    }
    let file = tmp.join("providers.json");
    fs::write(&file, r#"{"p": {"acme": "https://file.acme/v1"}}"#).unwrap();
    let spec = probe_spec(&format!(
        "{{flags: {{TP_FLAG: flagged}}, env: [TP_URL_A, TP_URL_B], path: {file:?}, key: \"p[$ctx.vendor]\", \
          default: {{api: \"https://api.default\", unknown: \"https://unknown.default\"}}}}"
    ));
    let at = |pairs: &[(&str, &str)], vendor: &str, plan: &str| endpoint_of(&spec, Some(&envs(pairs)), vendor, plan);
    assert_eq!(at(&[("TP_FLAG", "1"), ("TP_URL_A", "https://a.sess")], "acme", "api"), "flagged", "플래그 → env");
    std::env::set_var("TP_URL_A", "https://a.daemon");
    assert_eq!(at(&[("TP_URL_B", "https://b.sess")], "acme", "api"), "https://b.sess", "세션 routing_env → 데몬 환경");
    assert_eq!(at(&[], "acme", "api"), "https://a.daemon", "데몬 환경 → 파일");
    std::env::remove_var("TP_URL_A");
    assert_eq!(at(&[], "acme", "api"), "https://file.acme/v1", "파일 → default");
    assert_eq!(at(&[], "other", "api"), "https://api.default", "요금제별 default");
    assert_eq!(at(&[], "other", "subscription"), "https://unknown.default", "맞는 요금제가 없으면 unknown 칸");
    let bare = probe_spec("{env: [TP_URL_A]}");
    assert_eq!(endpoint_of(&bare, None, "sakana", "api"), "sakana", "마지막은 벤더를 맨 id로");
}

#[test]
fn legacy_endpoint_key_is_an_alias_in_user_override_only() {
    let (_g, root) = crate::test_home("legacy-endpoint");
    write_user_services(&root, "services:\n  old-ep:\n    roots: [\"~/x\"]\n    fields: {output: n}\n    endpoint: https://legacy.test/v1\n");
    assert!(load_report().warnings.is_empty(), "옛 키는 경고 없이 바뀐다");
    let spec = load_all_specs().into_iter().find(|s| s.name == "old-ep").expect("옛 형식도 로딩된다");
    assert_eq!(ServiceReader::new(spec).endpoint_for("", "acme"), "https://legacy.test/v1", "endpoint → endpoint_probe.default");
    assert!(uses_legacy("endpoint: https://x"), "adapters/에 넣으면 builtin_adapters_use_current_syntax가 막는다");
    assert!(!uses_legacy("endpoint_probe: {default: https://x}"));
    let raw = specs_from_yaml("services: {t: {roots: [\"~/x\"], endpoint: https://legacy.test/v1}}").pop().unwrap();
    assert_eq!(ServiceReader::new(raw).endpoint_for("", "acme"), "acme", "덮어쓰기 밖에서는 바꾸지 않는다");
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

/// 사용자 덮어쓰기처럼 옛 형식을 바꿔 봐서 무언가 바뀌면 옛 형식이다.
fn uses_legacy(text: &str) -> bool {
    let block: serde_yaml::Value = serde_yaml::from_str(text).unwrap();
    let mut upgraded = block.clone();
    upgrade_legacy(&mut upgraded);
    upgraded != block
}

#[test]
fn builtin_adapters_use_current_syntax() {
    for (id, text) in ADAPTERS {
        assert!(!uses_legacy(text), "adapters/{id}.yaml: 옛 형식(a.0.b, X: null, {{vendor}}, input_includes_cache)");
    }
}

#[test]
fn every_path_site_takes_an_expression() {
    let (_g, root) = crate::test_home("expr-sites");
    let data = root.join("data");
    write_user_services(
        &root,
        &format!(
            r#"services:
  t:
    roots: [{data:?}]
    match: {{data@json.role: assistant}}
    key: "message.id & requestId"
    fields:
      input: u.in
      cache_read: u.cached
      output: "u.a + u.b"
    context:
      model: [x.model, $.model]
      session: sid
    ctx_tokens: "u.in * 100"
    duration_ms: "t.end - t.start"
    subagent: meta@json.side
"#
        ),
    );
    let spec = load_all_specs().into_iter().find(|s| s.name == "t").expect("t가 로딩된다");
    let mut reader = ServiceReader::new(spec);
    let assistant = r#"{"role": "assistant"}"#;
    append(&data.join("s.jsonl"), &lines(&[
        json!({"data": assistant, "message": {"id": "m1"}, "requestId": "r1", "sid": "s-1", "model": "top",
               "u": {"in": 4, "cached": 6, "a": 2, "b": 3}, "t": {"start": 1000, "end": 1250},
               "meta": r#"{"side": false}"#}),
        json!({"data": assistant, "message": {"id": "m1"}, "requestId": "r1", "u": {"a": 5}}),
        json!({"data": r#"{"role": "user"}"#, "message": {"id": "m9"}, "requestId": "r9", "u": {"a": 70}}),
        json!({"data": assistant, "message": {"id": "m2"}, "requestId": "r1", "x": {"model": "inner"}, "model": "top",
               "u": {"a": 1}, "meta": r#"{"side": true}"#}),
    ]));
    let got = reader.poll();
    assert_eq!(got.len(), 2, "같은 `message.id & requestId`의 같은 값은 한 번, role user는 match 밖");
    assert_eq!(vec4(&got[0]), (4, 6, 0, 5), "output = u.a + u.b");
    assert_eq!((got[0].model.as_str(), got[0].session.as_str()), ("top", "s-1"), "x.model이 없으면 $.model");
    assert_eq!((got[0].ctx_tokens, got[0].duration_ms, got[0].subagent), (400, 250, false));
    assert_eq!(vec4(&got[1]), (0, 0, 0, 1));
    assert_eq!((got[1].model.as_str(), got[1].session.as_str()), ("inner", "s-1"), "session은 파일 문맥에서");
    assert_eq!((got[1].subagent, got[1].ctx_tokens), (true, 0));
}

#[test]
fn a_bad_expression_drops_only_that_service() {
    let (_g, root) = crate::test_home("bad-expr");
    write_user_services(
        &root,
        r#"services:
  bad:
    roots: ["~/x"]
    fields: {input: "a[", output: n}
  bad-match:
    roots: ["~/x"]
    match: {"a[": x}
    fields: {output: n}
  bad-probe:
    roots: ["~/x"]
    fields: {output: n}
    endpoint_probe: {path: "~/x.json", key: "a +"}
  good:
    roots: ["~/x"]
    fields: {output: "a + b"}
"#,
    );
    let names = loaded_names();
    for bad in ["bad", "bad-match", "bad-probe"] {
        assert!(!names.contains(&bad.to_string()), "{bad}");
    }
    assert!(names.contains(&"good".to_string()) && names.contains(&"claude-code".to_string()));
    let skipped = load_report().skipped;
    for (id, site) in [("bad", "fields.input"), ("bad-match", "match"), ("bad-probe", "endpoint_probe.key")] {
        assert!(skipped.iter().any(|(s, why)| s == id && why.starts_with(site)), "{id}: {skipped:?}");
    }
}

#[test]
fn legacy_forms_only_from_user_override() {
    let (_g, root) = crate::test_home("legacy");
    let data = root.join("old");
    let toml = root.join("config.toml");
    fs::write(&toml, "[model_providers.acme]\nbase_url = \"https://gw.acme.test/v1\"\n").unwrap();
    write_user_services(
        &root,
        &format!(
            "services:\n  old:\n    roots: [{data:?}]\n    match: {{kind: done, gone: null}}\n    fields: {{output: usage.0.out}}\n    endpoint_probe:\n      path: {toml:?}\n      key: model_providers.{{vendor}}.base_url\n  old-cache:\n    roots: [{:?}]\n    input_includes_cache: true\n    fields: {{input: i, cache_read: c}}\n  codex:\n    input_includes_cache: false\n",
            root.join("old-cache")
        ),
    );
    let spec = load_all_specs().into_iter().find(|s| s.name == "old").expect("옛 형식도 로딩된다");
    let mut reader = ServiceReader::new(spec);
    append(&data.join("s.jsonl"), &lines(&[
        json!({"kind": "done", "usage": [{"out": 7}]}),
        json!({"kind": "done", "gone": 1, "usage": [{"out": 100}]}),
        json!({"kind": "done", "gone": null, "usage": [{"out": 20}]}),
    ]));
    let outs: Vec<i64> = reader.poll().iter().map(|d| d.output_tokens).collect();
    assert_eq!(outs, [7, 20], "a.0.b는 번호, X: null은 없거나 null");
    assert_eq!(reader.endpoint_for("", "acme"), "https://gw.acme.test/v1", "{{vendor}} → [$ctx.vendor]");
    let specs = load_all_specs();
    let mut cache = ServiceReader::new(specs.iter().find(|s| s.name == "old-cache").unwrap().clone());
    append(&root.join("old-cache/s.jsonl"), &lines(&[json!({"i": 10, "c": 3})]));
    assert_eq!(vec4(&cache.poll()[0]), (7, 3, 0, 0), "input_includes_cache: true → [cache_read]");
    let codex = specs.iter().find(|s| s.name == "codex").unwrap();
    assert!(codex.input_includes.is_empty(), "false는 기본 어댑터의 목록을 비운다");

    for legacy in [
        "fields: {output: usage.0.out}",
        "match: {gone: null}",
        "endpoint_probe:\n  key: model_providers.{vendor}.base_url",
        "context:\n  model: [a.model, a.1.model]",
        "input_includes_cache: true",
        "input_includes_cache: false",
        "sources: [{fields: {output: usage.0.out}}]",
    ] {
        assert!(uses_legacy(legacy), "adapters/에 넣으면 builtin_adapters_use_current_syntax가 막는다: {legacy}");
    }
    assert!(!uses_legacy(
        "fields: {output: \"usage[0].out\"}\nmatch: {gone: {$exists: false}, $any: [{a: b}]}\nendpoint_probe:\n  key: model_providers[$ctx.vendor].base_url\ninput_includes: [cache_read]"
    ));
}

#[test]
fn builtin_probe_keys_take_the_vendor_from_ctx() {
    let (_g, root) = crate::test_home("probe-keys");
    std::env::remove_var("OPENAI_BASE_URL");
    let home = root.join("home");
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(home.join(".codex/config.toml"), "[model_providers.ollama]\nbase_url = \"http://localhost:11434/v1\"\n").unwrap();
    fs::create_dir_all(home.join(".config/opencode")).unwrap();
    fs::write(
        home.join(".config/opencode/opencode.json"),
        r#"{"provider": {"acme": {"options": {"baseURL": "https://gw.acme.test/v1"}}}}"#,
    )
    .unwrap();
    let mut codex = ServiceReader::new(spec_at("codex", &root));
    assert_eq!(codex.endpoint_for("", "ollama"), "http://localhost:11434/v1");
    let mut opencode = ServiceReader::new(spec_at("opencode", &root));
    assert_eq!(opencode.endpoint_for("", "acme"), "https://gw.acme.test/v1");
    assert_eq!(opencode.endpoint_for("", "nope"), "nope", "벤더 항목도 default도 없으면 벤더를 맨 id로");
}

#[test]
fn sources_inherit_and_override_service_level_keys() {
    let (_g, tmp) = crate::test_home("src-merge");
    let root = tmp.join("d");
    let mut reader = inline(
        &root,
        r#"patterns: ["*.jsonl"], match: {kind: call}, key: id, fields: {input: u.in, output: u.out},
        sources: [{fields: {output: u.out2}}, {patterns: ["*.log"], match: {src: b}, mode: cumulative, key: id2}]"#,
    );
    let [a, b] = &reader.spec.sources[..] else { panic!("소스 둘") };
    let text = |v: &serde_yaml::Value| v.as_str().unwrap_or_default().to_string();
    assert_eq!((text(&a.fields["input"]), text(&a.fields["output"])), ("u.in".into(), "u.out2".into()), "맵은 깊게");
    assert_eq!((a.patterns.clone(), text(&a.key), a.mode.as_str()), (vec!["*.jsonl".to_string()], "id".into(), "delta"));
    assert_eq!(
        (b.patterns.clone(), text(&b.key), b.mode.as_str()),
        (vec!["*.log".to_string()], "id2".into(), "cumulative"),
        "목록과 스칼라는 바꾼다"
    );
    assert_eq!(b.match_fields.len(), 2, "match도 맵이라 서비스 조건에 더해진다");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"kind": "call", "src": "b", "id": "1", "id2": "z", "u": {"in": 3, "out": 5, "out2": 7}}),
    ]));
    append(&root.join("b.log"), &lines(&[
        json!({"kind": "call", "src": "b", "id2": "x", "u": {"in": 10, "out": 20}}),
        json!({"src": "b", "id2": "y", "u": {"in": 100, "out": 1}}),
    ]));
    let got: Vec<_> = reader.poll().iter().map(vec4).collect();
    assert_eq!(got, [(3, 0, 0, 7), (10, 0, 0, 20)], "둘째 소스는 *.log만 읽고 kind도 본다");
}

#[test]
fn two_sources_share_one_ledger() {
    let (_g, tmp) = crate::test_home("src-ledger");
    let root = tmp.join("d");
    let mut reader = inline(
        &root,
        r#"key: id, fields: {output: out}, sources: [{patterns: ["*.jsonl"]}, {patterns: ["old/*.json"], format: json}]"#,
    );
    append(&root.join("s.jsonl"), &lines(&[json!({"id": "m1", "out": 5}), json!({"id": "m2", "out": 4})]));
    write_json(&root.join("old/m1.json"), &json!({"id": "m1", "out": 5}), 1);
    write_json(&root.join("old/m3.json"), &json!({"id": "m3", "out": 2}), 1);
    let got = reader.poll();
    assert_eq!((sum4(&got).3, calls(&got)), (5 + 4 + 2, 3), "두 소스에 같은 키가 있으면 한 번");
    write_json(&root.join("old/m1.json"), &json!({"id": "m1", "out": 8}), 2);
    let got = reader.poll();
    assert_eq!((got.len(), got[0].output_tokens, got[0].calls), (1, 3, 0), "다른 소스가 본 값에서 늘어난 만큼만");

    // cumulative 기준값도 서비스 안의 다른 파일 항목을 본다(소스가 달라도)
    let root = tmp.join("c");
    let mut reader = inline(
        &root,
        r#"mode: cumulative, key: sid, fields: {output: out}, sources: [{patterns: ["*.jsonl"]}, {patterns: ["*.log"]}]"#,
    );
    append(&root.join("a.jsonl"), &lines(&[json!({"sid": "s", "out": 100})]));
    append(&root.join("b.log"), &lines(&[json!({"sid": "s", "out": 130})]));
    let got = reader.poll();
    assert_eq!(got.iter().map(|d| d.output_tokens).collect::<Vec<_>>(), [100, 30]);
    assert_eq!(calls(&got), 1);
}

#[test]
fn empty_entry_is_the_service_defaults() {
    let (_g, tmp) = crate::test_home("src-empty");
    let root = tmp.join("d");
    append(&root.join("s.jsonl"), &lines(&[
        json!({"id": "a", "sid": "s-1", "out": 5}),
        json!({"id": "b", "out": 6}),
        json!({"id": "a", "out": 5}),
    ]));
    let body = "key: id, fields: {output: out}, context: {session: sid}";
    let seen = |got: Vec<TokenDelta>| -> Vec<_> { got.iter().map(|d| (vec4(d), d.session.clone(), d.calls)).collect() };
    let plain = seen(inline(&root, body).poll());
    let mut reader = inline(&root, &format!("{body}, sources: [{{}}]"));
    assert_eq!(reader.spec.sources.len(), 1);
    let got = seen(reader.poll());
    assert_eq!(got, plain);
    assert_eq!(got.len(), 2);
}

#[test]
fn service_only_key_in_source_is_rejected() {
    let (_g, root) = crate::test_home("src-only");
    write_user_services(
        &root,
        r#"services:
  a:
    roots: ["~/x"]
    fields: {output: n}
    sources: [{plan: api}]
  b:
    roots: ["~/x"]
    sources: [{fields: {output: n}}, {fields: {input: "a["}}]
  c:
    roots: ["~/x"]
    sources: [{roots: "not-a-list"}]
  d:
    roots: ["~/x"]
    sources: [{fields: {output: n}, colour: red}]
"#,
    );
    let names = loaded_names();
    for bad in ["a", "b", "c"] {
        assert!(!names.contains(&bad.to_string()), "{bad}");
    }
    assert!(names.contains(&"d".to_string()) && names.contains(&"claude-code".to_string()));
    let report = load_report();
    for (id, site) in [("a", "sources[0].plan"), ("b", "sources[1].fields.input"), ("c", "sources[0].roots")] {
        assert!(report.skipped.iter().any(|(s, why)| s == id && why.starts_with(site)), "{id}: {:?}", report.skipped);
    }
    assert_eq!(report.warnings, vec!["d: unknown key sources[0].colour".to_string()], "모르는 키는 경고뿐");
}

#[test]
fn sources_report_under_the_service_name() {
    let (_g, root) = crate::test_home("src-name");
    let data = root.join("data");
    write_user_services(
        &root,
        &format!(
            "services:\n  mine:\n    label: Mine\n    roots: [{data:?}]\n    key: id\n    fields: {{output: out}}\n    sources: [{{match: {{t: a}}}}, {{match: {{t: b}}}}]\n"
        ),
    );
    let specs = load_all_specs();
    assert_eq!(specs.iter().filter(|s| s.name == "mine").count(), 1, "소스마다 서비스가 생기지 않는다");
    let mut reader = ServiceReader::new(specs.into_iter().find(|s| s.name == "mine").unwrap());
    append(&data.join("s.jsonl"), &lines(&[json!({"t": "a", "id": 1, "out": 3}), json!({"t": "b", "id": 2, "out": 4})]));
    assert_eq!(reader.files().len(), 1, "두 소스가 같은 파일을 읽어도 목록에는 한 번");
    let got = reader.poll();
    assert_eq!(got.iter().map(|d| (d.service.as_str(), d.output_tokens)).collect::<Vec<_>>(), [("mine", 3), ("mine", 4)]);
    assert!(reader.spec.sources.iter().all(|s| s.name == "mine" && s.label == "Mine"));
}

#[test]
fn unset_var_root_is_dropped_not_absolute() {
    let (_g, tmp) = crate::test_home("root-unset");
    std::env::remove_var("TM_ROOT_NOPE");
    // 빈 문자열로 펴면 `<tmp>/d`가 되어 아래 파일을 읽는다(B4)
    append(&tmp.join("d/s.jsonl"), &lines(&[json!({"id": "a", "out": 5})]));
    let root = format!("${{TM_ROOT_NOPE}}{}", tmp.join("d").display());
    let spec = specs_from_yaml(&format!("services: {{t: {{roots: [{root:?}], key: id, fields: {{output: out}}}}}}"))
        .pop()
        .unwrap();
    let mut reader = ServiceReader::new(spec);
    assert!(reader.files().is_empty());
    assert!(reader.poll().is_empty());
}

#[test]
fn same_root_twice_reads_once() {
    let (_g, tmp) = crate::test_home("root-twice");
    std::env::remove_var("TM_ROOT_A");
    let home = tmp.join("home");
    append(&home.join("x/s.jsonl"), &lines(&[json!({"id": "a", "out": 5}), json!({"id": "b", "out": 2})]));
    let spec = specs_from_yaml(
        r#"services: {t: {roots: ["${TM_ROOT_A:-~/x}", "~/x", "~/./x/"], fields: {output: out}}}"#,
    )
    .pop()
    .unwrap();
    let mut reader = ServiceReader::new(spec);
    assert_eq!(reader.files(), vec![home.join("x/s.jsonl")]);
    assert_eq!(sum4(&reader.poll()).3, 7, "키가 없어도 한 번");
}

#[test]
fn excluded_journal_is_not_read() {
    let (_g, tmp) = crate::test_home("root-exclude");
    let root = tmp.join("d");
    append(&root.join("p/s.jsonl"), &lines(&[json!({"id": "a", "out": 5})]));
    append(&root.join("p/subagents/workflows/w1/journal.jsonl"), &lines(&[json!({"id": "b", "out": 70})]));
    append(&root.join("p/subagents/a.jsonl"), &lines(&[json!({"id": "c", "out": 2})]));
    let mut reader = inline(
        &root,
        r#"key: id, fields: {output: out}, exclude: ["**/subagents/workflows/*/journal.jsonl"]"#,
    );
    assert_eq!(reader.files().len(), 2);
    assert_eq!(sum4(&reader.poll()).3, 7);
}

#[test]
fn roots_from_registry_patterns_find_files() {
    let (_g, tmp) = crate::test_home("root-registry");
    let home = tmp.join("home");
    let app = tmp.join("code/C#/a[p]p");
    let other = tmp.join("code/other");
    append(&app.join(".crush/s.jsonl"), &lines(&[json!({"id": "a", "out": 5})]));
    append(&app.join(".crush/deep/x.jsonl"), &lines(&[json!({"id": "b", "out": 70})]));
    append(&other.join("data/s.jsonl"), &lines(&[json!({"id": "c", "out": 2})]));
    fs::create_dir_all(home.join("reg")).unwrap();
    fs::write(
        home.join("reg/projects.json"),
        json!({"projects": [
            {"path": app, "data_dir": ".crush"},
            {"path": other, "dir": "data"},
            {"path": home, "data_dir": "."},
        ]})
        .to_string(),
    )
    .unwrap();
    let text = r#"services:
  t:
    key: id
    fields: {output: out}
    roots_from:
      - {file: "~/reg/projects.json", each: projects, path: [data_dir, dir], base: path, patterns: ["*.jsonl"]}
"#;
    write_user_services(&tmp, text);
    assert_eq!(load_report().roots_from_dropped, 1, "doctor는 버린 수만 센다");
    let spec = specs_from_yaml(text).pop().unwrap();
    let mut reader = ServiceReader::new(spec);
    let mut files = reader.files();
    files.sort();
    assert_eq!(files, vec![app.join(".crush/s.jsonl"), other.join("data/s.jsonl")], "항목 패턴만, HOME은 버린다");
    assert_eq!(sum4(&reader.poll()).3, 7);
}

#[test]
fn brace_root_is_a_validation_error() {
    let (_g, root) = crate::test_home("root-brace");
    write_user_services(
        &root,
        r#"services:
  a:
    roots: ["~/.{claude,codex}"]
    fields: {output: n}
  b:
    roots: ["~/a/*/b"]
    fields: {output: n}
  c:
    roots: ["~/x"]
    patterns: ["*.{json,jsonl}"]
    fields: {output: n}
  d:
    roots: ["~/x"]
    exclude: ["{a,b}/*.jsonl"]
    fields: {output: n}
  e:
    fields: {output: n}
    roots_from: [{file: "~/r.json", path: p, patterns: ["**/crush.db"]}]
  f:
    roots: ["${TM_X"]
    fields: {output: n}
  g:
    roots: ["${TM_ROOT_G:-~/x}"]
    exclude: ["**/journal.jsonl"]
    fields: {output: n}
    roots_from: [{file: "~/r.json", each: projects, path: dir, patterns: ["*.db"]}]
"#,
    );
    let names = loaded_names();
    let report = load_report();
    for (id, site) in [("a", "roots"), ("b", "roots"), ("c", "patterns"), ("d", "exclude"), ("e", "roots_from"), ("f", "roots")] {
        assert!(!names.contains(&id.to_string()), "{id}");
        assert!(report.skipped.iter().any(|(s, why)| s == id && why.starts_with(site)), "{id}: {:?}", report.skipped);
    }
    assert!(names.contains(&"g".to_string()), "{:?}", report.skipped);
}

#[test]
fn overlapping_roots_across_services_are_reported() {
    let (_g, tmp) = crate::test_home("root-overlap");
    std::env::set_var("TM_ROOT_SHARED", tmp.join("home/.pi/agent"));
    fs::create_dir_all(tmp.join("home/.pi/agent/sessions")).unwrap();
    let specs = specs_from_yaml(
        r#"services:
  pi: {roots: ["~/.other", "${TM_ROOT_SHARED}/sessions"], fields: {output: n}}
  omp: {roots: ["~/.pi/agent/sessions/"], fields: {output: n}}
  mine: {roots: ["~/.mine", "~/.mine/sub"], fields: {output: n}}
  up: {fields: {output: n}, sources: [{roots: ["~/.q"]}, {roots: ["~/.pi"]}]}
"#,
    );
    let got = overlaps(&specs);
    std::env::remove_var("TM_ROOT_SHARED");
    let pair = |a: &str, i, b: &str, j| [(a.to_string(), i), (b.to_string(), j)];
    assert_eq!(
        got,
        vec![pair("pi", 1, "omp", 0), pair("pi", 1, "up", 1), pair("omp", 0, "up", 1)],
        "서비스 사이만, 같은 서비스 안(mine)은 아니다. 조상 루트도 겹친다"
    );
    assert!(load_report().overlaps.is_empty(), "기본 어댑터끼리는 겹치지 않는다");
}

#[test]
fn record_time_goes_on_the_delta() {
    let (_g, tmp) = crate::test_home("record-time");
    let root = tmp.join("logs");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"id": "a", "n": 5, "ts": "2026-09-20T10:00:00Z"}),
        json!({"id": "b", "n": 3, "time": {"created": 1_789_898_400_123_i64}}),
        json!({"id": "c", "n": 1}),
    ]));
    let mut reader = inline(&root, "key: id, timestamp: [ts, time.created], fields: {output: n}");
    let at: Vec<f64> = reader.poll().iter().map(|d| d.at).collect();
    // 2026-01-01T00:00Z = 1767225600, 9월 20일은 262일 뒤, 10시 = +36000
    assert_eq!(at, [1_789_898_400.0, 1_789_898_400.123, 0.0], "밀리초는 크기로 가르고, 시각이 없으면 0(지금)");
}

#[test]
fn first_is_the_first_parsed_timestamp_even_without_match() {
    let (_g, tmp) = crate::test_home("first-ts");
    let root = tmp.join("logs");
    let path = root.join("fork.jsonl");
    append(&path, &lines(&[
        json!({"type": "note"}),
        json!({"type": "session", "ts": "2026-09-20T10:00:00Z"}),
        json!({"type": "m", "id": "x", "n": 1, "ts": "2026-09-19T00:00:00Z"}),
    ]));
    let mut reader = inline(&root, "match: {type: m}, key: id, timestamp: ts, fields: {output: n}");
    let got = reader.poll();
    assert_eq!(got.iter().map(|d| d.at).collect::<Vec<_>>(), [1_789_776_000.0]);
    let first = reader.sources[0].first.get(&super::reader::path_key(&path)).copied();
    assert_eq!(first, Some(1_789_898_400.0), "match 밖의 머리 줄 시각이 파일의 첫 시각이다(포크 시각)");
}

#[test]
fn doctor_counts_match_drops_and_field_hits() {
    let (_g, tmp) = crate::test_home("doctor-counts");
    let root = tmp.join("logs");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"type": "u"}),
        json!({"type": "m", "id": "1", "i": 1, "o": 2}),
        json!({"type": "m", "id": "2", "o": 3}),
    ]));
    append(&root.join("a.jsonl"), "not json\n");
    append(&root.join("a.jsonl"), &lines(&[json!({"type": "m", "id": "3", "i": "4", "o": 1})]));
    let mut reader = inline(&root, "match: {type: m}, key: id, fields: {input: i, output: o}");
    reader.poll();
    let st = &reader.stats;
    assert_eq!((st.records, st.dropped_by_match, st.matched), (4, 1, 3), "깨진 줄은 레코드가 아니다");
    assert_eq!(reader.field_hits(), vec![("input", 2.0 / 3.0), ("output", 1.0)], "적힌 필드만, 숫자 글자도 값이다");
}

#[test]
fn verified_defaults_to_true_and_is_a_service_key() {
    let (_g, root) = crate::test_home("verified");
    write_user_services(
        &root,
        "services:\n  a:\n    roots: [\"~/a\"]\n    fields: {output: n}\n  b:\n    roots: [\"~/b\"]\n    verified: false\n    timestamp: ts\n    fields: {output: n}\n",
    );
    let specs = load_all_specs();
    let get = |n: &str| specs.iter().find(|s| s.name == n).unwrap().verified;
    assert!(get("a") && !get("b"));
    assert!(load_report().warnings.is_empty(), "verified·timestamp는 아는 키");
}

// --- SQLite 소스(F1, 스펙 5절) ---

fn sql_db(path: &Path, setup: &str) -> rusqlite::Connection {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(path).unwrap();
    conn.execute_batch(setup).unwrap();
    conn
}

/// 테스트는 2초 간격을 기다리지 않는다.
fn rewind(reader: &mut ServiceReader) {
    for src in &mut reader.sources {
        src.db.values_mut().for_each(|d| d.queried = 0.0);
    }
}

const MSG_TABLE: &str = "CREATE TABLE message(id TEXT, session_id TEXT, updated INTEGER, data TEXT);";

const MSG_SPEC: &str = r#"format: sqlite, patterns: ["x.db"], key: id, cursor: updated, default_model: dflt,
    query: "SELECT id, session_id, updated, data FROM message WHERE updated >= ?1",
    match: {data@json.role: assistant}, cost_usd: data@json.cost,
    fields: {input: data@json.tokens.input, cache_read: data@json.tokens.cache.read,
             cache_write: data@json.tokens.cache.write, output: data@json.tokens.output},
    context: {session: session_id, model: data@json.modelID}"#;

fn msg(conn: &rusqlite::Connection, id: &str, updated: i64, data: Value) {
    conn.execute(
        "INSERT INTO message VALUES (?1, 's', ?2, ?3)",
        rusqlite::params![id, updated, data.to_string()],
    )
    .unwrap();
}

fn out_msg(out: i64, cost: f64) -> Value {
    json!({"role": "assistant", "tokens": {"output": out}, "cost": cost})
}

#[test]
fn sqlite_source_reads_rows_as_records() {
    let (_g, tmp) = crate::test_home("sql-rows");
    let root = tmp.join("oc");
    let conn = sql_db(&root.join("x.db"), MSG_TABLE);
    let ins = "INSERT INTO message VALUES (?1, ?2, ?3, ?4)";
    conn.execute(ins, rusqlite::params!["m1", "s1", 1000, json!({"role": "assistant", "modelID": "claude-opus-4-5",
        "tokens": {"input": 10, "output": 20, "cache": {"read": 5, "write": 1}}, "cost": 0.5}).to_string()]).unwrap();
    conn.execute(ins, rusqlite::params!["m2", "s1", 1001, json!({"role": "user"}).to_string()]).unwrap();
    conn.execute(ins, rusqlite::params!["m3", "s2", 1002, json!({"role": "assistant",
        "tokens": {"input": 1, "output": 2, "cache": {"read": 0, "write": 0}}}).to_string()]).unwrap();
    let mut reader = inline(&root, MSG_SPEC);
    let got = reader.poll();
    let seen: Vec<_> = got.iter().map(|d| (vec4(d), d.model.as_str(), d.session.as_str(), d.cost_usd)).collect();
    assert_eq!(
        seen,
        [((10, 5, 1, 20), "claude-opus-4-5", "s1", Some(0.5)), ((1, 0, 0, 2), "dflt", "s2", None)],
        "행은 앞 행의 문맥을 물려받지 않는다"
    );
    assert_eq!((reader.stats.records, reader.stats.dropped_by_match), (3, 1));
}

#[test]
fn row_update_zero_to_final_counts_growth_once() {
    let (_g, tmp) = crate::test_home("sql-update");
    let root = tmp.join("oc");
    let conn = sql_db(&root.join("x.db"), MSG_TABLE);
    msg(&conn, "m1", 1000, out_msg(0, 0.0));
    let mut reader = inline(&root, MSG_SPEC);
    assert!(reader.poll().is_empty(), "스트림 시작의 0 행");
    conn.execute("UPDATE message SET updated = 1030, data = ?1 WHERE id = 'm1'", [out_msg(50, 0.0).to_string()])
        .unwrap();
    rewind(&mut reader);
    let got = reader.poll();
    assert_eq!((got.len(), got[0].output_tokens, got[0].calls), (1, 50, 1));
    rewind(&mut reader);
    assert!(reader.poll().is_empty(), "도장이 그대로면 쿼리하지 않는다");
}

#[test]
fn cursor_boundary_reread_counts_tokens_and_cost_once() {
    let (_g, tmp) = crate::test_home("sql-cursor");
    let root = tmp.join("oc");
    let conn = sql_db(&root.join("x.db"), MSG_TABLE);
    let ms = 1_790_000_000_000i64;
    msg(&conn, "a", ms, out_msg(10, 0.125));
    msg(&conn, "b", ms + 30_000, out_msg(5, 0.25));
    let mut reader = inline(&root, MSG_SPEC);
    let got = reader.poll();
    assert_eq!((sum4(&got).3, got.iter().filter_map(|d| d.cost_usd).sum::<f64>()), (15, 0.375));
    assert_eq!(reader.sources[0].db.values().next().unwrap().cursor, Some(ms + 30_000));
    // 다음 쿼리는 ms − 30초부터: a·b를 다시 읽지만 장부가 거른다
    msg(&conn, "c", ms + 40_000, out_msg(7, 0.0625));
    rewind(&mut reader);
    let got = reader.poll();
    assert_eq!(reader.stats.records, 2 + 3, "경계 행을 다시 읽었다");
    assert_eq!((got.len(), got[0].output_tokens, got[0].cost_usd, calls(&got)), (1, 7, Some(0.0625), 1));
}

#[test]
fn wal_commit_without_db_mtime_change_is_seen() {
    let (_g, tmp) = crate::test_home("sql-wal");
    let root = tmp.join("oc");
    let db = root.join("x.db");
    let conn = sql_db(&db, "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;");
    conn.execute_batch(MSG_TABLE).unwrap();
    msg(&conn, "a", 1000, out_msg(3, 0.0));
    let mut reader = inline(&root, MSG_SPEC);
    assert_eq!(sum4(&reader.poll()).3, 3);
    let before = fs::metadata(&db).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    msg(&conn, "b", 1001, out_msg(4, 0.0));
    let after = fs::metadata(&db).unwrap();
    assert_eq!((before.len(), before.modified().ok()), (after.len(), after.modified().ok()), "본 파일은 그대로");
    rewind(&mut reader);
    let got = reader.poll();
    assert_eq!((got.len(), got[0].output_tokens), (1, 4), "-wal의 커밋을 본다");
}

#[test]
fn inode_change_or_shrink_resets_the_cursor() {
    use std::os::unix::fs::MetadataExt;
    let (_g, tmp) = crate::test_home("sql-reset");
    let root = tmp.join("oc");
    let db = root.join("x.db");
    let conn = sql_db(&db, &format!("{MSG_TABLE} CREATE TABLE pad(b BLOB);"));
    conn.execute("INSERT INTO pad VALUES (zeroblob(200000))", []).unwrap();
    msg(&conn, "a", 2_000_000, out_msg(1, 0.0));
    let mut reader = inline(&root, MSG_SPEC);
    assert_eq!(sum4(&reader.poll()).3, 1);

    // 줄었다(같은 inode): 커서보다 한참 이른 행도 읽는다
    let ino = fs::metadata(&db).unwrap().ino();
    let size = fs::metadata(&db).unwrap().len();
    conn.execute_batch("DELETE FROM pad;").unwrap();
    msg(&conn, "c", 1_000, out_msg(3, 0.0));
    conn.execute_batch("VACUUM;").unwrap();
    let meta = fs::metadata(&db).unwrap();
    assert!(meta.ino() == ino && meta.len() < size, "같은 파일이 줄었다");
    rewind(&mut reader);
    let got = reader.poll();
    assert_eq!(got.iter().map(|d| d.output_tokens).collect::<Vec<_>>(), [3]);

    // 다른 파일(inode)로 바뀌었다
    drop(conn);
    let other = root.join("new.db");
    let conn = sql_db(&other, MSG_TABLE);
    msg(&conn, "b", 1_000, out_msg(4, 0.0));
    drop(conn);
    fs::rename(&other, &db).unwrap();
    rewind(&mut reader);
    let got = reader.poll();
    assert_eq!(got.iter().map(|d| d.output_tokens).collect::<Vec<_>>(), [4]);
}

#[test]
fn busy_skips_the_poll_and_counts_after_three() {
    let (_g, tmp) = crate::test_home("sql-busy");
    let root = tmp.join("oc");
    let conn = sql_db(&root.join("x.db"), MSG_TABLE);
    msg(&conn, "a", 1000, out_msg(9, 0.0));
    conn.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let mut reader = inline(&root, MSG_SPEC);
    for n in [0, 0, 1] {
        rewind(&mut reader);
        assert!(reader.poll().is_empty(), "바쁘면 이번 폴을 건너뛴다");
        assert_eq!(reader.stats.sqlite_busy, n, "세 번 이어지면 한 번 센다");
    }
    conn.execute_batch("COMMIT;").unwrap();
    rewind(&mut reader);
    assert_eq!(sum4(&reader.poll()).3, 9, "다음 폴에 다시 한다");
}

#[test]
fn sqlite_without_key_or_query_is_rejected() {
    let (_g, root) = crate::test_home("sql-check");
    write_user_services(
        &root,
        "services:\n  nq:\n    roots: [\"~/x\"]\n    format: sqlite\n    key: id\n    fields: {output: n}\n  nk:\n    roots: [\"~/x\"]\n    format: sqlite\n    query: SELECT 1\n    fields: {output: n}\n  ok:\n    roots: [\"~/x\"]\n    format: sqlite\n    key: id\n    query: [SELECT a FROM t, SELECT 1]\n    cursor: updated\n    fields: {output: n}\n",
    );
    let names = loaded_names();
    assert!(!names.contains(&"nq".to_string()) && !names.contains(&"nk".to_string()));
    assert!(names.contains(&"ok".to_string()));
    let report = load_report();
    let skipped = report.skipped;
    assert!(skipped.iter().any(|(id, why)| id == "nq" && why.starts_with("query")), "{skipped:?}");
    assert!(skipped.iter().any(|(id, why)| id == "nk" && why.starts_with("key")), "{skipped:?}");
    assert!(report.warnings.is_empty(), "query·cursor는 아는 키: {:?}", report.warnings);
    let ok = load_all_specs().into_iter().find(|s| s.name == "ok").unwrap();
    assert_eq!((ok.query.len(), ok.cursor.as_str()), (2, "updated"));
}

#[test]
fn wal_shm_journal_files_do_not_match_patterns() {
    let (_g, tmp) = crate::test_home("sql-patterns");
    let root = tmp.join("oc");
    fs::create_dir_all(&root).unwrap();
    for f in ["opencode.db", "opencode.db-wal", "opencode.db-shm", "opencode-dev.db", "opencode-dev.db-journal"] {
        fs::write(root.join(f), "").unwrap();
    }
    let reader = inline(&root, r#"format: sqlite, key: id, query: "SELECT 1", patterns: ["opencode.db", "opencode-*.db"]"#);
    let mut names: Vec<String> =
        reader.files().iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["opencode-dev.db", "opencode.db"]);
}

// ---- 펼치기(F4)와 레코드 밖 문맥(F6), Task 2.8 ----

fn outs(got: &[TokenDelta]) -> Vec<i64> {
    got.iter().map(|d| d.output_tokens).collect()
}

#[test]
fn each_over_array_map_and_root_array() {
    let (_g, tmp) = crate::test_home("each-shapes");
    let root = tmp.join("logs");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"id": "r1", "calls": [{"n": 3}, {"n": 4}]}),
        json!({"id": "r2", "calls": {"x": {"n": 5}, "y": {"n": 6}}}),
    ]));
    let mut reader = inline(&root, r#"each: calls, key: "$.id & $index", fields: {output: n}"#);
    let got = reader.poll();
    assert_eq!(outs(&got), [3, 4, 5, 6], "배열은 원소마다, 맵은 값마다");
    assert_eq!(calls(&got), 4, "원소가 각자 호출이다");

    let json_root = tmp.join("json");
    write_json(&json_root.join("chat.json"), &json!([{"ts": 1, "n": 7}, {"ts": 2, "n": 8}]), 1);
    let mut reader = inline(&json_root, r#"format: json, patterns: ["*.json"], each: "$", key: ts, fields: {output: n}"#);
    assert_eq!(outs(&reader.poll()), [7, 8], "`each: \"$\"`는 최상위 배열");
    write_json(&json_root.join("chat.json"), &json!([{"ts": 1, "n": 7}, {"ts": 2, "n": 8}, {"ts": 3, "n": 9}]), 2);
    assert_eq!(outs(&reader.poll()), [9], "통파일을 다시 읽어도 본 원소는 키 장부가 거른다");
}

#[test]
fn each_key_binds_model() {
    let (_g, tmp) = crate::test_home("each-key");
    let root = tmp.join("grok");
    let turn = |prompt: &str, usage: Value| {
        json!({"params": {"update": {"sessionUpdate": "turn_completed", "prompt_id": prompt,
                                     "usage": {"modelUsage": usage}}}})
    };
    append(&root.join("s.jsonl"), &lines(&[turn("p1", json!({
        "grok-4": {"inputTokens": 100, "cachedReadTokens": 40, "outputTokens": 10},
        "grok-3-mini": {"inputTokens": 7, "outputTokens": 2},
    }))]));
    let mut reader = inline(&root, r#"each: params.update.usage.modelUsage,
        match: {$.params.update.sessionUpdate: turn_completed}, key: "$.params.update.prompt_id & $key",
        fields: {input: inputTokens, cache_read: cachedReadTokens, output: outputTokens}, context: {model: $key}"#);
    let mut got: Vec<(String, (i64, i64, i64, i64))> = reader.poll().iter().map(|d| (d.model.clone(), vec4(d))).collect();
    got.sort();
    assert_eq!(got, [("grok-3-mini".into(), (7, 0, 0, 2)), ("grok-4".into(), (100, 40, 0, 10))]);
}

#[test]
fn match_runs_per_element_with_dollar_outer() {
    let (_g, tmp) = crate::test_home("each-match");
    let root = tmp.join("logs");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"type": "turn", "id": "t1", "items": [{"kind": "usage", "n": 3}, {"kind": "note", "n": 50}]}),
        json!({"type": "draft", "id": "t2", "items": [{"kind": "usage", "n": 70}]}),
    ]));
    let mut reader = inline(&root, r#"each: items, match: {$.type: turn, kind: usage}, key: "$.id & $index", fields: {output: n}"#);
    assert_eq!(outs(&reader.poll()), [3]);
    assert_eq!((reader.stats.records, reader.stats.matched, reader.stats.dropped_by_match), (2, 1, 2), "match는 원소마다 센다");
}

#[test]
fn no_elements_no_deltas() {
    let (_g, tmp) = crate::test_home("each-empty");
    let root = tmp.join("logs");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"id": "a", "items": [], "n": 9}),
        json!({"id": "b", "n": 9}),
        json!({"id": "c", "items": 4, "n": 9}),
    ]));
    let mut reader = inline(&root, r#"each: items, key: "$.id & $index", fields: {output: "$.n"}"#);
    assert!(reader.poll().is_empty(), "빈 목록, 없는 경로, 스칼라는 원소가 없다");
    assert_eq!(reader.stats.matched, 0);
}

#[test]
fn context_learned_before_each_is_the_fallback() {
    let (_g, tmp) = crate::test_home("each-ctx");
    let root = tmp.join("logs");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"id": "r1", "model": "outer-m", "sid": "s-1", "items": [{"n": 1, "model": "inner-m"}, {"n": 2}]}),
        json!({"id": "r2", "items": [{"n": 3}]}),
    ]));
    let mut reader = inline(&root, r#"each: items, key: "$.id & $index", fields: {output: n},
        context: {model: model, session: sid}"#);
    let got: Vec<(i64, String, String)> =
        reader.poll().iter().map(|d| (d.output_tokens, d.model.clone(), d.session.clone())).collect();
    assert_eq!(got, [
        (1, "inner-m".into(), "s-1".into()),
        (2, "outer-m".into(), "s-1".into()),
        (3, "outer-m".into(), "s-1".into()),
    ], "원소 값이 먼저, 없으면 펼치기 전에 배운 값(앞 레코드 것 포함)");
}

#[test]
fn each_without_key_is_rejected() {
    let (_g, root) = crate::test_home("each-no-key");
    write_user_services(
        &root,
        "services:\n  nk:\n    roots: [\"~/x\"]\n    each: items\n    fields: {output: n}\n  ok:\n    roots: [\"~/x\"]\n    each: items\n    key: \"$.id & $index\"\n    fields: {output: n}\n",
    );
    let names = loaded_names();
    assert!(!names.contains(&"nk".to_string()) && names.contains(&"ok".to_string()));
    let report = load_report();
    assert!(report.skipped.iter().any(|(id, why)| id == "nk" && why.starts_with("key")), "{:?}", report.skipped);
    assert!(report.warnings.is_empty(), "each는 아는 키: {:?}", report.warnings);
}

#[test]
fn cumulative_each_key_needs_key_or_index() {
    let (_g, root) = crate::test_home("each-cumulative");
    write_user_services(
        &root,
        "services:\n  bad:\n    roots: [\"~/x\"]\n    mode: cumulative\n    each: byModel\n    key: $.sid\n    fields: {output: n}\n  byk:\n    roots: [\"~/x\"]\n    mode: cumulative\n    each: byModel\n    key: \"$.sid & $key\"\n    fields: {output: n}\n  byi:\n    roots: [\"~/x\"]\n    mode: cumulative\n    each: byModel\n    key: [\"$.sid & $index\"]\n    fields: {output: n}\n",
    );
    let names = loaded_names();
    assert!(!names.contains(&"bad".to_string()));
    assert!(names.contains(&"byk".to_string()) && names.contains(&"byi".to_string()));
    let skipped = load_report().skipped;
    assert!(skipped.iter().any(|(id, why)| id == "bad" && why.starts_with("key")), "{skipped:?}");
}

#[test]
fn context_when_learns_only_from_header() {
    let (_g, tmp) = crate::test_home("ctx-when");
    let root = tmp.join("pi");
    append(&root.join("a.jsonl"), &lines(&[
        json!({"type": "session", "id": "sess-A"}),
        json!({"type": "message", "id": "m1", "n": 3}),
        json!({"type": "message", "id": "m2", "n": 4}),
    ]));
    let mut reader = inline(&root, r#"match: {type: message}, key: id, fields: {output: n},
        context: {session: {path: id, when: {type: session}}}"#);
    let got: Vec<String> = reader.poll().iter().map(|d| d.session.clone()).collect();
    assert_eq!(got, ["sess-A", "sess-A"], "모든 줄에 id가 있어도 머리 줄에서만 배운다");
    append(&root.join("b.jsonl"), &lines(&[json!({"type": "message", "id": "m3", "n": 5})]));
    assert_eq!(reader.poll()[0].session, "", "머리 줄이 없는 파일은 배운 값이 없다");
}

#[test]
fn context_with_bad_when_or_keys_is_rejected() {
    let (_g, root) = crate::test_home("ctx-when-bad");
    write_user_services(
        &root,
        "services:\n  a:\n    roots: [\"~/x\"]\n    fields: {output: n}\n    context: {session: {path: id, if: {type: s}}}\n  b:\n    roots: [\"~/x\"]\n    fields: {output: n}\n    context: {session: {when: {type: s}}}\n",
    );
    let skipped = load_report().skipped;
    for id in ["a", "b"] {
        assert!(skipped.iter().any(|(s, why)| s == id && why.starts_with("context.session")), "{id}: {skipped:?}");
    }
}

#[test]
fn file_vars_give_session_from_path() {
    let (_g, tmp) = crate::test_home("file-vars");
    let root = tmp.join("kimi");
    let path = root.join("proj-1/sess-42/agent/wire.jsonl");
    append(&path, &lines(&[json!({"id": "a", "n": 2})]));
    let mut reader = inline(&root, r#"key: "$file.path & id", fields: {output: n},
        context: {session: "$file.dir[-2]", cwd: "$root & $file.stem & $file.name"}"#);
    let got = reader.poll();
    assert_eq!(got[0].session, "sess-42");
    assert_eq!(got[0].cwd, format!("{}|wire|wire.jsonl", root.display()));
}

#[test]
fn sidecar_value_and_mtime_reload() {
    let (_g, tmp) = crate::test_home("sidecar");
    let root = tmp.join("gemini/tmp");
    let chat = root.join("h1/chats/session.jsonl");
    let side = root.join("h1/.project_root");
    fs::create_dir_all(side.parent().unwrap()).unwrap();
    let set_side = |text: &str, tick: u64| {
        fs::write(&side, text).unwrap();
        let when = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + tick);
        fs::File::options().write(true).open(&side).unwrap().set_modified(when).unwrap();
    };
    set_side("/work/p1\n", 1);
    fs::write(root.join("../workspaces.json"), r#"{"workspaces": {"h1": {"name": "alpha"}}, // 주석
    }"#).unwrap();
    append(&chat, &lines(&[json!({"id": "a", "n": 1})]));
    let mut reader = inline(&root, r#"key: id, fields: {output: n}, patterns: ["*/chats/*.jsonl"],
        sidecars: {root: ../.project_root, ws: "$root/../workspaces.json", gone: ../nope.json},
        context: {cwd: [cwd, $side.root], session: "$side.ws.workspaces[$file.dir[-2]].name", effort: $side.gone}"#);
    let got = reader.poll();
    assert_eq!((got[0].cwd.as_str(), got[0].session.as_str(), got[0].effort.as_str()), ("/work/p1", "alpha", ""));
    set_side("/work/p2", 2);
    append(&chat, &lines(&[json!({"id": "b", "n": 1}), json!({"id": "c", "n": 1, "cwd": "/own"})]));
    let got: Vec<String> = reader.poll().iter().map(|d| d.cwd.clone()).collect();
    assert_eq!(got, ["/work/p2", "/own"], "mtime이 바뀌면 다시 읽고, 레코드 값이 먼저다");
}

#[test]
fn sqlite_rows_have_no_file_context_memory() {
    let (_g, tmp) = crate::test_home("sql-no-ctx");
    let root = tmp.join("oc");
    let conn = sql_db(&root.join("x.db"), "CREATE TABLE t(id TEXT, kind TEXT, sid TEXT, n INTEGER);");
    conn.execute_batch(
        "INSERT INTO t VALUES ('h', 'session', 'S1', 0); INSERT INTO t VALUES ('a', 'msg', 'S2', 3); INSERT INTO t VALUES ('b', 'msg', NULL, 4);",
    )
    .unwrap();
    let mut reader = inline(&root, r#"format: sqlite, patterns: ["x.db"], key: id, query: "SELECT * FROM t ORDER BY rowid",
        match: {kind: msg}, fields: {output: n}, context: {session: sid, cwd: {path: sid, when: {kind: session}}}"#);
    let got: Vec<(String, String)> = reader.poll().iter().map(|d| (d.session.clone(), d.cwd.clone())).collect();
    assert_eq!(got, [("S2".into(), "".into()), ("".into(), "".into())], "행은 제 열만 쓴다(머리 행도 잇지 않는다)");
}

// ── 읽기 상태 되살리기와 문턱(F8, B2, 스펙 4.3–4.5, 13절 상태 테스트) ──

const HOUR: f64 = 3600.0;
const TS_KEYED: &str = "key: id, timestamp: ts, fields: {output: out}";

/// 레코드 `{id, out, ts}`. `ago`는 지금부터 몇 초 전인지.
fn rec_at(id: &str, out: i64, ago: f64) -> Value {
    json!({"id": id, "out": out, "ts": (now_secs() - ago) as i64})
}

fn set_mtime(path: &Path, ago: f64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs_f64(ago);
    fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
}

/// 커밋(`export` → `stage` → `finish`)하고 새 리더를 `gate` 문턱으로 되살린다(데몬 재시작).
fn restart(old: &mut ServiceReader, root: &Path, body: &str, gate: f64) -> ServiceReader {
    let store = checkpoint::Store { dir: root.with_extension("readers") };
    let (mut file, keys) = old.export();
    file.seq = store.load("t").map_or(1, |f| f.seq + 1);
    store.stage("t", &file, &keys).unwrap();
    store.finish("t").unwrap();
    let mut r = inline(root, body);
    r.set_gate(gate);
    r.restore(store.load("t"), store.keys("t", file.seq));
    r
}

#[test]
fn first_run_emits_nothing() {
    let (_g, tmp) = crate::test_home("first-run");
    // JSONL: 48시간 안은 끝까지 배우고, 그 밖은 끝 위치의 차가운 항목
    let root = tmp.join("jsonl");
    append(&root.join("new.jsonl"), &lines(&[rec_at("a", 5, 60.0)]));
    append(&root.join("old.jsonl"), &lines(&[rec_at("b", 7, 3.0 * 86_400.0)]));
    set_mtime(&root.join("old.jsonl"), 3.0 * 86_400.0);
    let mut r = inline(&root, TS_KEYED);
    r.prime();
    assert!(r.poll().is_empty());
    let (file, _) = r.export();
    let old = &file.files[&checkpoint::path_key(&root.join("old.jsonl"))];
    assert!(old.head.is_none() && old.off > 0, "오래된 파일은 짧은 항목");
    append(&root.join("new.jsonl"), &lines(&[rec_at("a", 9, 0.0)]));
    assert_eq!(outs(&r.poll()), [4], "배운 값에서 늘어난 만큼만");

    // JSON 통파일: 나이와 상관없이 한 번 읽어 배운다(누적 스냅샷의 평생 합계를 내지 않는다)
    let root = tmp.join("json");
    let path = root.join("s.json");
    write_json(&path, &json!({"sid": "s", "out": 1000}), 1);
    let mut r = inline(&root, r#"format: json, patterns: ["*.json"], mode: cumulative, key: sid, fields: {output: out}"#);
    r.prime();
    assert!(r.poll().is_empty());
    write_json(&path, &json!({"sid": "s", "out": 1010}), 2);
    assert_eq!(outs(&r.poll()), [10]);

    // SQLite: 커서 없이 끝까지 배우고 커서를 최댓값으로, 재시작 뒤에도 그대로
    let root = tmp.join("oc");
    let conn = sql_db(&root.join("x.db"), MSG_TABLE);
    msg(&conn, "m1", 1000, out_msg(50, 0.5));
    let mut r = inline(&root, MSG_SPEC);
    r.prime();
    rewind(&mut r);
    assert!(r.poll().is_empty());
    let mut r = restart(&mut r, &root, MSG_SPEC, now_secs() - 600.0);
    assert_eq!(r.sources[0].db.values().next().and_then(|d| d.cursor), Some(1000));
    assert!(r.poll().is_empty(), "되살린 장부와 커서: 다시 읽어도 내지 않는다");
    msg(&conn, "m2", 2000, out_msg(3, 0.0));
    rewind(&mut r);
    let got = r.poll();
    assert_eq!((outs(&got), got.iter().map(|d| d.cost_usd).collect::<Vec<_>>()), (vec![3], vec![None]));
}

#[test]
fn known_file_resumes_at_offset() {
    let (_g, tmp) = crate::test_home("resume");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    append(&path, &lines(&[rec_at("a", 5, 60.0)]));
    let mut r = inline(&root, TS_KEYED);
    assert_eq!(outs(&r.poll()), [5]);
    // 꺼진 동안 붙은 줄은 문턱보다 이른 시각이어도 새 기록이다. 7일보다 이른 것만 배운다.
    append(&path, &lines(&[rec_at("b", 6, 2.0 * HOUR), rec_at("c", 7, 8.0 * 86_400.0), rec_at("d", 8, 30.0)]));
    let mut r = restart(&mut r, &root, TS_KEYED, now_secs() - 600.0);
    assert_eq!(outs(&r.poll()), [6, 8]);
    assert_eq!(r.sources[0].offset[&super::reader::path_key(&path)], fs::metadata(&path).unwrap().len());
}

#[test]
fn head_hash_under_4k_appends_only_new_lines() {
    let (_g, tmp) = crate::test_home("head-4k");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    append(&path, &lines(&[rec_at("a", 5, 60.0), rec_at("b", 6, 60.0)]));
    let mut r = inline(&root, TS_KEYED);
    assert_eq!(outs(&r.poll()), [5, 6]);
    let size = fs::metadata(&path).unwrap().len();
    let mut r = restart(&mut r, &root, TS_KEYED, now_secs());
    let (h, n) = r.sources[0].head[&super::reader::path_key(&path)];
    assert_eq!((n as u64, h), (size, ledger::fnv1a64(&fs::read(&path).unwrap())), "4 KB 미만이면 파일 전체");
    // 줄이 붙어도 같은 길이만큼 다시 해시하므로 head가 같다
    append(&path, &lines(&[rec_at("c", 7, 3.0 * HOUR)]));
    assert_eq!(outs(&r.poll()), [7]);
    append(&path, &lines(&[rec_at("d", 8, 3.0 * HOUR)]));
    assert_eq!(outs(&r.poll()), [8], "head 길이가 자라도 이어 읽는다");
}

#[test]
fn rewritten_file_is_unknown_and_old_records_only_learn() {
    let (_g, tmp) = crate::test_home("rewritten");
    let root = tmp.join("d");
    let (old, new) = (HOUR, 10.0);
    let (a, b, c) = (root.join("a.jsonl"), root.join("b.jsonl"), root.join("c.jsonl"));
    append(&a, &lines(&[rec_at("a1", 5, old)]));
    append(&b, &lines(&[rec_at("b1", 5, old), rec_at("b2", 6, old), rec_at("b3", 6, old)]));
    append(&c, &lines(&[rec_at("c1", 5, old)]));
    let mut r = inline(&root, TS_KEYED);
    assert_eq!(r.poll().len(), 5);
    let mut r = restart(&mut r, &root, TS_KEYED, now_secs() - 600.0);
    // ino 바뀜: 새 파일을 이름 바꿔 덮는다(더 길다)
    let tmp_a = root.join("a.tmp");
    fs::write(&tmp_a, lines(&[rec_at("a1", 5, old), rec_at("a2", 6, old), rec_at("a3", 7, new)])).unwrap();
    fs::rename(&tmp_a, &a).unwrap();
    // size < off
    fs::write(&b, lines(&[rec_at("b4", 9, old), rec_at("b5", 8, new)])).unwrap();
    // 같은 inode에 머리만 바뀜(첫 줄 길이가 같아 off부터 읽으면 c7, c2를 새 줄로 센다)
    fs::write(&c, lines(&[rec_at("c9", 9, old), rec_at("c7", 7, old), rec_at("c2", 6, new)])).unwrap();
    let mut got = outs(&r.poll());
    got.sort();
    assert_eq!(got, [6, 7, 8], "모르는 파일: 문턱 뒤 레코드만 낸다");
}

#[test]
fn cold_entry_relearns_up_to_offset_then_emits_tail() {
    let (_g, tmp) = crate::test_home("cold");
    let root = tmp.join("d");
    let path = root.join("old.jsonl");
    let body = "mode: cumulative, key: sid, fields: {output: out}";
    append(&path, &lines(&[json!({"sid": "s1", "out": 100}), json!({"sid": "s2", "out": 200})]));
    set_mtime(&path, 3.0 * 86_400.0);
    let mut r = inline(&root, body);
    r.prime();
    let mut r = restart(&mut r, &root, body, now_secs() - 600.0);
    assert!(r.sources[0].blind.contains(&super::reader::path_key(&path)), "짧은 항목은 차가운 항목으로 돌아온다");
    assert!(r.poll().is_empty());
    // B2: s2는 배운 기준값에서 잇고(평생 합계 200을 내지 않음), 새 스트림 s3는 문턱(파일 mtime)으로 낸다
    append(&path, &lines(&[json!({"sid": "s2", "out": 250}), json!({"sid": "s3", "out": 5})]));
    assert_eq!(outs(&r.poll()), [50, 5]);
    append(&path, &lines(&[json!({"sid": "s1", "out": 130})]));
    assert_eq!(outs(&r.poll()), [30]);
}

#[test]
fn unknown_file_records_after_threshold_emit_before_learn() {
    let (_g, tmp) = crate::test_home("unknown-gate");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    let mut r = inline(&root, TS_KEYED);
    r.set_gate(now_secs() - 600.0);
    append(&path, &lines(&[rec_at("x", 5, HOUR), rec_at("y", 6, 60.0)]));
    assert_eq!(outs(&r.poll()), [6], "문턱 전 레코드는 배우기만 한다");
    append(&path, &lines(&[rec_at("x", 9, HOUR)]));
    assert_eq!(outs(&r.poll()), [4], "배운 키는 늘어난 만큼만");
}

#[test]
fn timestampless_record_uses_file_mtime() {
    let (_g, tmp) = crate::test_home("no-ts");
    let root = tmp.join("d");
    append(&root.join("a.jsonl"), &lines(&[json!({"id": "a", "out": 5})]));
    set_mtime(&root.join("a.jsonl"), HOUR);
    append(&root.join("b.jsonl"), &lines(&[json!({"id": "b", "out": 6})]));
    let mut r = inline(&root, TS_KEYED);
    r.set_gate(now_secs() - 600.0);
    assert_eq!(outs(&r.poll()), [6]);
}

#[test]
fn backlog_older_than_7d_only_learns() {
    let (_g, tmp) = crate::test_home("backlog");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    append(&path, &lines(&[rec_at("a", 1, 60.0)]));
    let mut r = inline(&root, TS_KEYED);
    r.prime();
    append(&path, &lines(&[rec_at("old", 5, 8.0 * 86_400.0), rec_at("mid", 6, 6.0 * 86_400.0)]));
    assert_eq!(outs(&r.poll()), [6], "아는 파일이어도 7일보다 이르면 배우기만");
    let mut open = inline(&root, TS_KEYED);
    assert_eq!(outs(&open.poll()), [1, 5, 6], "문턱이 없으면(doctor --since) 7일 상한도 없다");
}

#[test]
fn new_cumulative_stream_in_known_file_uses_threshold() {
    let (_g, tmp) = crate::test_home("cum-new-stream");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    let body = "mode: cumulative, key: sid, timestamp: ts, fields: {output: out}";
    let cum = |sid: &str, out: i64, ago: f64| json!({"sid": sid, "out": out, "ts": (now_secs() - ago) as i64});
    append(&path, &lines(&[cum("s1", 100, 60.0)]));
    let mut r = inline(&root, body);
    r.prime();
    r.set_gate(now_secs() - 600.0);
    append(&path, &lines(&[cum("s2", 50, HOUR), cum("s3", 7, 0.0), cum("s1", 120, 0.0), cum("s2", 55, 0.0)]));
    assert_eq!(outs(&r.poll()), [7, 20, 5], "문턱 전에 나타난 새 스트림은 기준값만 잡는다");
}

#[test]
fn replay_gate_skips_records_before_header_minus_60() {
    let (_g, tmp) = crate::test_home("replay-gate");
    let root = tmp.join("d");
    let header = json!({"type": "session", "ts": (now_secs() - 100.0) as i64});
    let m = |id: &str, out: i64, ago: f64| json!({"type": "m", "id": id, "out": out, "ts": (now_secs() - ago) as i64});
    let recs = [header, m("k1", 1, 220.0), m("k2", 2, 130.0), m("k3", 3, 50.0)];
    append(&root.join("s.jsonl"), &lines(&recs));
    for (gate, want) in [("true", vec![2, 3]), ("false", vec![1, 2, 3])] {
        let mut r = inline(&root, &format!("match: {{type: m}}, replay_gate: {gate}, {TS_KEYED}"));
        r.set_gate(now_secs() - 600.0);
        assert_eq!(outs(&r.poll()), want, "replay_gate: {gate}");
    }
}

#[test]
fn replay_gate_seconds_skips_burst_at_fork_instant() {
    let (_g, tmp) = crate::test_home("replay-burst");
    let root = tmp.join("sessions");
    let row = |ts: &str, total: i64, last: i64| json!({"timestamp": ts, "type": "event_msg",
        "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": total},
                                                   "last_token_usage": {"input_tokens": last}}}});
    append(&root.join("fork.jsonl"), &lines(&[
        json!({"timestamp": "2026-05-05T21:51:57.991Z", "type": "session_meta", "payload": {"id": "child"}}),
        row("2026-05-05T21:51:57.994Z", 116000, 73000), // 재생: 포크 순간
        row("2026-05-05T21:51:58.948Z", 116500, 500),   // 재생: 머리 줄 + 1초 안
        row("2026-05-05T21:51:59.253Z", 117500, 1000),  // 자식의 첫 호출
    ]));
    let no_gate = |body: &str| {
        let text = format!("services: {{t: {{roots: [{root:?}], {body}}}}}");
        ServiceReader::with_opts(specs_from_yaml(&text).pop().unwrap(), ReadOpts::no_gate())
    };
    let mut reader = no_gate(r#"timestamp: timestamp, replay_gate: 1, key: payload.info.total_token_usage,
        match: {payload.type: token_count}, fields: {input: payload.info.last_token_usage.input_tokens}"#);
    let got = reader.poll();
    assert_eq!(sum4(&got), (1000, 0, 0, 0), "머리 줄 시각 + 1초 안의 레코드는 배우기만 한다");
    let mut pi = no_gate(r#"timestamp: timestamp, replay_gate: true, key: payload.info.total_token_usage,
        match: {payload.type: token_count}, fields: {input: payload.info.last_token_usage.input_tokens}"#);
    assert_eq!(sum4(&pi.poll()), (74500, 0, 0, 0), "true는 지금처럼 머리 줄 − 60초");
}

#[test]
fn orphan_fork_is_filtered_by_header_time() {
    let (_g, tmp) = crate::test_home("orphan-fork");
    let root = tmp.join("d");
    // 부모 파일 없는 pi 포크: 머리 줄(포크 시각) 뒤에 원본 항목이 원래 시각으로 복사된다
    append(&root.join("fork.jsonl"), &lines(&[
        json!({"type": "session", "ts": "2026-09-20T10:00:00Z"}),
        json!({"type": "m", "id": "p1", "out": 40, "ts": "2026-09-19T09:00:00Z"}),
        json!({"type": "m", "id": "p2", "out": 50, "ts": "2026-09-20T09:58:59Z"}),
        json!({"type": "m", "id": "n1", "out": 7, "ts": "2026-09-20T10:00:10Z"}),
    ]));
    let spec = specs_from_yaml(&format!(
        "services: {{t: {{roots: [{:?}], match: {{type: m}}, replay_gate: true, {TS_KEYED}}}}}",
        root
    ))
    .pop()
    .unwrap();
    let mut r = ServiceReader::with_opts(spec, ReadOpts::no_gate());
    assert_eq!(outs(&r.poll()), [7], "문턱이 없어도(하네스) 복제 문턱은 거른다");
}

#[test]
fn keyless_copy_is_not_counted_twice_across_restart() {
    let (_g, tmp) = crate::test_home("keyless-restart");
    let root = tmp.join("d");
    let body = "fields: {output: out}";
    let recs = [json!({"n": 1, "out": 5}), json!({"n": 2, "out": 6})];
    append(&root.join("a.jsonl"), &lines(&recs));
    let mut r = inline(&root, body);
    assert_eq!(outs(&r.poll()), [5, 6]);
    let mut r = restart(&mut r, &root, body, now_secs() - 600.0);
    let mut copy = recs.to_vec();
    copy.push(json!({"n": 3, "out": 7}));
    append(&root.join("b.jsonl"), &lines(&copy));
    assert_eq!(outs(&r.poll()), [7], "되살린 장부가 복사본을 거른다");
}

#[test]
fn measure_off_period_is_not_counted() {
    let (_g, tmp) = crate::test_home("measure-off");
    let root = tmp.join("d");
    let path = root.join("s.jsonl");
    append(&path, &lines(&[rec_at("a", 1, 2.0 * HOUR)]));
    let mut r = inline(&root, TS_KEYED);
    r.prime();
    append(&path, &lines(&[rec_at("b", 2, HOUR)]));
    assert_eq!(outs(&r.poll()), [2]);
    // 끔(1시간): 그동안 쓴 기록. 켤 때 readers를 지우고 문턱 = 켠 시각으로 처음 실행.
    append(&path, &lines(&[rec_at("c", 3, 1800.0)]));
    append(&root.join("n.jsonl"), &lines(&[rec_at("d", 4, 1200.0)]));
    let on = now_secs() - 60.0;
    let mut r = inline(&root, TS_KEYED);
    r.set_gate(on);
    r.restore(None, Vec::new());
    assert!(r.poll().is_empty());
    // 켠 뒤에 나타난 파일에도 끈 동안의 레코드는 배우기만
    append(&root.join("late.jsonl"), &lines(&[rec_at("e", 5, 600.0), rec_at("f", 6, 0.0)]));
    append(&path, &lines(&[rec_at("g", 7, 0.0)]));
    let mut got = outs(&r.poll());
    got.sort();
    assert_eq!(got, [6, 7]);
}
