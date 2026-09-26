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
fn enabled_services_declare_token_fields() {
    for spec in default_specs() {
        let views = if spec.sources.is_empty() { vec![&spec] } else { spec.sources.iter().collect() };
        for view in views {
            let has = |f: &str| view.fields.get(f).is_some_and(|v| !v.is_null());
            assert!(has("output") || has("input"), "{}", spec.name);
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

/// 로더처럼 파싱한 프로브 키로 부른다.
fn plan_of(spec: &ServiceSpec) -> String {
    resolve_plan(spec, Compiled::new(spec).unwrap().plan_key.as_ref())
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
    assert_eq!(opencode.endpoint_for("", "nope"), "", "벤더 항목이 없으면 비어 있다(opencode는 default가 없다)");
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
