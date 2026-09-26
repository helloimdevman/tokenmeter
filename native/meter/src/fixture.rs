//! fixture 하네스(스펙 13절): `tests/fixtures/<id>/`를 가짜 HOME에 펼쳐 그 어댑터로 읽는다.
//! `tests/fixtures.rs`와 `adapter check`가 같이 쓴다. 프로세스 전역 환경(HOME 등)을 바꾼다.

use crate::engine::route_labels;
use crate::watch::{load_all_specs, specs_from_yaml_opts, ServiceReader, TokenDelta, ADAPTERS};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

static LOCK: Mutex<()> = Mutex::new(());

/// 하네스가 가짜 디렉터리로 정하는 변수. 어댑터 원문의 다른 변수는 모두 비운다.
const OWN_VARS: [&str; 6] = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "TOKENMETER_HOME",
];

/// `steps[n]`에 쓸 수 있는 칸. 모르는 칸은 오타로 보고 실패한다.
const STEP_KEYS: [&str; 13] = [
    "calls",
    "input",
    "cache_read",
    "cache_write",
    "output",
    "cost_usd",
    "by_model",
    "sessions",
    "project",
    "vendor",
    "plan",
    "route",
    "model_label",
];

/// 델타 하나가 더하는 칸(`calls`는 Σ `delta.calls`).
const SUMS: [&str; 5] = ["calls", "input", "cache_read", "cache_write", "output"];

/// `dir`의 fixture를 가짜 HOME에 펼치고 `id` 어댑터(`spec_text`가 있으면 그 YAML)로 읽어
/// 단계마다 누계를 돌려준다. `step-2/`가 있으면 둘째 폴 전에 덮어쓴다.
pub fn run(id: &str, spec_text: Option<&str>, dir: &Path) -> Result<Vec<Value>, String> {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "tokenmeter-fixture-{id}-{}-{nanos}",
        std::process::id()
    ));
    let got = run_in(id, spec_text, dir, &tmp);
    let _ = fs::remove_dir_all(&tmp);
    got
}

fn run_in(id: &str, spec_text: Option<&str>, dir: &Path, tmp: &Path) -> Result<Vec<Value>, String> {
    let (home, state) = (tmp.join("home"), tmp.join("state"));
    for d in [&home, &state] {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    for text in ADAPTERS.iter().map(|(_, t)| *t).chain(spec_text) {
        for var in env_names(text) {
            if !OWN_VARS.contains(&var) {
                std::env::remove_var(var);
            }
        }
    }
    for (var, path) in [
        ("HOME", home.clone()),
        ("XDG_CONFIG_HOME", home.join(".config")),
        ("XDG_DATA_HOME", home.join(".local/share")),
        ("XDG_STATE_HOME", home.join(".local/state")),
        ("XDG_CACHE_HOME", home.join(".cache")),
        ("TOKENMETER_HOME", state.clone()),
    ] {
        std::env::set_var(var, path);
    }
    lay(&dir.join("files"), &home)?;
    lay(&dir.join("tokenmeter-home"), &state)?;

    let spec = match spec_text {
        Some(text) => {
            let block: serde_yaml::Value =
                serde_yaml::from_str(text).map_err(|e| format!("{id}: {e}"))?;
            let root = serde_yaml::Mapping::from_iter([(
                "services".into(),
                serde_yaml::Mapping::from_iter([(id.into(), block)]).into(),
            )]);
            let text = serde_yaml::to_string(&root).map_err(|e| e.to_string())?;
            specs_from_yaml_opts(&text, true).into_iter().next()
        }
        None => load_all_specs().into_iter().find(|s| s.name == id),
    }
    .ok_or_else(|| format!("adapter {id} did not load"))?;

    // ponytail: prime() 없이 폴 — 모든 레코드가 새것이다. 2.9가 ReadOpts::no_gate()로 바꾼다.
    let mut reader = ServiceReader::new(spec);
    let mut deltas = reader.poll();
    let mut steps = vec![totals(&deltas)];
    let step2 = dir.join("step-2");
    if step2.is_dir() {
        // 같은 순간에 다시 쓴 파일을 폴이 놓치지 않게 mtime을 민다(잠자기 없이).
        let later = SystemTime::now() + Duration::from_secs(2);
        for path in lay(&step2, &home)? {
            let wal = PathBuf::from(format!("{}-wal", path.display()));
            for p in [path, wal].iter().filter(|p| p.exists()) {
                fs::File::options()
                    .write(true)
                    .open(p)
                    .and_then(|f| f.set_modified(later))
                    .map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
        reader.forget_sqlite_gap();
        deltas.extend(reader.poll());
        steps.push(totals(&deltas));
    }
    Ok(steps)
}

/// `src` 아래 파일을 `dst`로 복사한다. `*.sql`은 확장자를 뗀 경로의 SQLite DB에 실행한다
/// (없으면 만들고, 있으면 행 갱신). 쓴 경로를 돌려준다.
fn lay(src: &Path, dst: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    walk(src, &mut files);
    let mut out = Vec::new();
    for file in files {
        let rel = file.strip_prefix(src).map_err(|e| e.to_string())?;
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", rel.display());
        let target = dst.join(rel);
        fs::create_dir_all(target.parent().unwrap_or(dst)).map_err(|e| err(&e))?;
        if file.extension().is_some_and(|x| x == "sql") {
            let db = target.with_extension("");
            let sql = fs::read_to_string(&file).map_err(|e| err(&e))?;
            rusqlite::Connection::open(&db)
                .and_then(|c| c.execute_batch(&sql))
                .map_err(|e| err(&e))?;
            out.push(db);
        } else {
            fs::copy(&file, &target).map_err(|e| err(&e))?;
            out.push(target);
        }
    }
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// 어댑터 원문의 환경 변수 이름: `$VAR`·`${VAR…}`, 그리고 밑줄이 든 대문자 낱말
/// (`plan_probe.env` 목록, `flags` 키). 실행하는 셸의 값이 fixture 결과를 바꾸지 않게 한다.
fn env_names(text: &str) -> Vec<&str> {
    let b = text.as_bytes();
    let word = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !word(b[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && word(b[i]) {
            i += 1;
        }
        let w = &text[start..i];
        let upper = b[start].is_ascii_uppercase()
            && w.bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_');
        let after_dollar = start > 0 && matches!(b[start - 1], b'$' | b'{');
        if upper && (w.contains('_') || after_dollar) {
            out.push(w);
        }
    }
    out
}

/// 지금까지의 델타 누계(`cost_usd`는 로그에 적힌 비용의 합). `project`·`vendor`·`plan`·`route`·`model_label`은 모든 델타가
/// 같으면 그 값, 아니면 서로 다른 값의 목록이다(기대값의 문자열과 맞지 않아 실패한다).
fn totals(deltas: &[TokenDelta]) -> Value {
    let mut sum = [0i64; 5];
    let mut by_model: BTreeMap<&str, [i64; 5]> = BTreeMap::new();
    let mut sessions = BTreeSet::new();
    let mut seen: [BTreeSet<String>; 5] = Default::default();
    for d in deltas {
        let v = [
            i64::from(d.calls),
            d.input_tokens,
            d.cache_read,
            d.cache_write,
            d.output_tokens,
        ];
        let m = by_model.entry(d.model.as_str()).or_default();
        for i in 0..5 {
            sum[i] += v[i];
            m[i] += v[i];
        }
        if !d.session.is_empty() {
            sessions.insert(d.session.as_str());
        }
        let [_, route, _, model] = route_labels(d);
        for (set, value) in seen
            .iter_mut()
            .zip([&d.project, &d.vendor, &d.plan, &route, &model])
        {
            set.insert(value.clone());
        }
    }
    let row = |v: &[i64; 5]| -> Map<String, Value> {
        SUMS.iter()
            .zip(v)
            .map(|(k, n)| (k.to_string(), json!(n)))
            .collect()
    };
    let mut out = row(&sum);
    let logged: f64 = deltas.iter().map(|d| d.cost_usd.unwrap_or(0.0)).sum();
    out.insert("cost_usd".into(), json!(logged));
    out.insert(
        "by_model".into(),
        json!(by_model
            .iter()
            .map(|(m, v)| (m.to_string(), Value::Object(row(v))))
            .collect::<Map<_, _>>()),
    );
    out.insert("sessions".into(), json!(sessions));
    for (key, set) in ["project", "vendor", "plan", "route", "model_label"]
        .into_iter()
        .zip(seen)
    {
        let value = if set.len() == 1 {
            json!(set.first())
        } else {
            json!(set)
        };
        out.insert(key.into(), value);
    }
    Value::Object(out)
}

/// `expected.json`의 `steps`만 본다(`source`, `differs`, `ccusage`는 무시). 단계 수가 같아야 하고
/// 단계마다 적힌 칸만 맞춘다. `by_model`은 모델 집합이 같고 모델마다 적힌 칸이 같아야 한다.
pub fn compare(expected: &Value, got: &[Value]) -> Result<(), String> {
    let steps = expected
        .get("steps")
        .and_then(Value::as_array)
        .ok_or("expected.json: no steps")?;
    let mut bad = Vec::new();
    if steps.len() != got.len() {
        bad.push(format!("{} steps, got {}", steps.len(), got.len()));
    }
    for (n, (want, got)) in steps.iter().zip(got).enumerate() {
        let Some(want) = want.as_object() else {
            bad.push(format!("step {}: not an object", n + 1));
            continue;
        };
        for (key, w) in want {
            if !STEP_KEYS.contains(&key.as_str()) {
                bad.push(format!("step {}: unknown field {key}", n + 1));
                continue;
            }
            let g = &got[key.as_str()];
            let ok = if key == "by_model" {
                same_models(w, g)
            } else if key == "cost_usd" {
                matches!((w.as_f64(), g.as_f64()), (Some(w), Some(g)) if (w - g).abs() <= 1e-9)
            } else {
                w == g
            };
            if !ok {
                bad.push(format!("step {}: {key} want {w} got {g}", n + 1));
            }
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad.join("; "))
    }
}

fn same_models(want: &Value, got: &Value) -> bool {
    let (Some(want), Some(got)) = (want.as_object(), got.as_object()) else {
        return false;
    };
    want.len() == got.len()
        && want.iter().all(|(model, fields)| {
            fields.as_object().is_some_and(|f| {
                f.iter()
                    .all(|(k, v)| got.get(model).and_then(|m| m.get(k)) == Some(v))
            })
        })
}

/// `root`(tests/fixtures) 아래 텍스트 파일과 `.zst`에서 실사용 로그의 흔적을 찾는다(스펙 13절).
/// 걸린 파일과 이유. `root/allow-long.txt`에 한 줄씩 적은 문자열은 길어도 넘어간다.
pub fn personal_data(root: &Path) -> Vec<String> {
    let allow: HashSet<String> = fs::read_to_string(root.join("allow-long.txt"))
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    let mut files = Vec::new();
    walk(root, &mut files);
    let mut bad = Vec::new();
    for path in files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "allow-long.txt" {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else { continue };
        let (kind, bytes) = match ext(name) {
            "zst" => match unzstd(&bytes) {
                Some(plain) => (ext(name.trim_end_matches(".zst")), plain),
                None => {
                    bad.push(format!("{rel}: unreadable .zst"));
                    continue;
                }
            },
            "jsonl" | "json" | "sql" | "txt" | "toml" | "yaml" => (ext(name), bytes),
            _ => continue,
        };
        let text = String::from_utf8_lossy(&bytes);
        let mut why = BTreeSet::new();
        leaks(&text, &mut why);
        // 기대값의 `source`·`differs`는 손으로 쓴 설명이라 길어도 된다.
        let long_ok = name == "expected.json";
        for s in strings(&text, kind) {
            check(&s, &allow, long_ok, &mut why);
        }
        bad.extend(why.into_iter().map(|w| format!("{rel}: {w}")));
    }
    bad
}

fn ext(name: &str) -> &str {
    name.rsplit_once('.').map(|(_, x)| x).unwrap_or("")
}

/// 프레임이 여럿이어도 끝까지 푼다. 풀리지 않으면 `None`.
fn unzstd(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut src = bytes;
    let mut out = Vec::new();
    while !src.is_empty() {
        let mut frame = ruzstd::decoding::StreamingDecoder::new(&mut src).ok()?;
        frame.read_to_end(&mut out).ok()?;
    }
    Some(out)
}

/// 파일에서 검사할 문자열: JSON은 키와 문자열 값, SQL은 `'…'` 글자, 그 밖은 공백으로 나눈 낱말.
fn strings(text: &str, kind: &str) -> Vec<String> {
    let mut out = Vec::new();
    match kind {
        "json" => match serde_json::from_str::<Value>(text) {
            Ok(v) => json_strings(&v, &mut out),
            Err(_) => out.extend(text.split_whitespace().map(str::to_string)),
        },
        "jsonl" => {
            for line in text.lines() {
                match serde_json::from_str::<Value>(line) {
                    Ok(v) => json_strings(&v, &mut out),
                    Err(_) => out.extend(line.split_whitespace().map(str::to_string)),
                }
            }
        }
        "sql" => {
            // 'it''s' 는 붙은 두 조각으로 나뉘어도 검사에는 상관없다.
            out.extend(text.split('\'').skip(1).step_by(2).map(str::to_string));
        }
        _ => out.extend(text.split_whitespace().map(str::to_string)),
    }
    out
}

fn json_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|x| json_strings(x, out)),
        Value::Object(map) => {
            for (k, x) in map {
                out.push(k.clone());
                json_strings(x, out);
            }
        }
        _ => {}
    }
}

/// 문자열 하나. JSON으로 풀리면(OpenCode `data` 열) 풀어서 안을 본다.
fn check(s: &str, allow: &HashSet<String>, long_ok: bool, why: &mut BTreeSet<String>) {
    leaks(s, why);
    if let Ok(inner @ (Value::Object(_) | Value::Array(_))) = serde_json::from_str::<Value>(s) {
        let mut parts = Vec::new();
        json_strings(&inner, &mut parts);
        for p in parts {
            check(&p, allow, long_ok, why);
        }
        return;
    }
    let n = s.chars().count();
    if n > 80 && !long_ok && !allow.contains(s) {
        let head: String = s.chars().take(24).collect();
        why.insert(format!("string over 80 chars ({n}): {head}…"));
    }
}

fn leaks(s: &str, why: &mut BTreeSet<String>) {
    for p in ["/Users/", "/home/"] {
        if s.contains(p) {
            why.insert(p.into());
        }
    }
    // 낱말 머리에서만 본다("task-1"의 sk-는 키가 아니다). sk-ant-는 sk-에 든다.
    let b = s.as_bytes();
    for p in ["sk-", "ghp_", "xai-"] {
        if s.match_indices(p)
            .any(|(i, _)| i == 0 || !b[i - 1].is_ascii_alphanumeric())
        {
            why.insert(p.into());
        }
    }
    if has_email(s) {
        why.insert("email address".into());
    }
}

/// `a@b.cc` 꼴. `ccusage@20.0.25`처럼 끝이 숫자인 것은 주소가 아니다.
fn has_email(s: &str) -> bool {
    let b = s.as_bytes();
    let local = |c: u8| c.is_ascii_alphanumeric() || b"._%+-".contains(&c);
    let host = |c: u8| c.is_ascii_alphanumeric() || c == b'.' || c == b'-';
    s.match_indices('@').any(|(i, _)| {
        let end = b[i + 1..]
            .iter()
            .position(|c| !host(*c))
            .map_or(b.len(), |n| i + 1 + n);
        let domain = s[i + 1..end].trim_end_matches('.');
        i > 0
            && local(b[i - 1])
            && domain.rsplit_once('.').is_some_and(|(h, tld)| {
                !h.is_empty() && tld.len() >= 2 && tld.bytes().all(|c| c.is_ascii_alphabetic())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step2_sql_update_is_read_on_the_second_poll() {
        // run()은 HOME을 바꾼다: 같은 프로세스의 test_home() 테스트와 겹치지 않게 그 잠금도 쥔다.
        let _home = crate::TEST_HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("tokenmeter-step2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("files/d")).unwrap();
        fs::create_dir_all(dir.join("step-2/d")).unwrap();
        fs::write(
            dir.join("files/d/x.db.sql"),
            "CREATE TABLE t(id TEXT, updated INTEGER, n INTEGER); INSERT INTO t VALUES ('a', 1000, 5);",
        )
        .unwrap();
        fs::write(dir.join("step-2/d/x.db.sql"), "UPDATE t SET n = 9, updated = 2000;").unwrap();
        let spec = r#"{roots: ["~/d"], patterns: ["x.db"], format: sqlite, key: id, cursor: updated,
                       query: "SELECT id, updated, n FROM t WHERE updated >= ?1", fields: {output: n}}"#;
        let got = run("step2-sql", Some(spec), &dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(
            (got[0]["output"].clone(), got[1]["output"].clone()),
            (json!(5), json!(9))
        );
    }

    #[test]
    fn personal_data_finds_paths_keys_emails_and_long_strings() {
        let root = std::env::temp_dir().join(format!("tokenmeter-pd-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("x")).unwrap();
        let long_a = "a".repeat(81);
        let long_b = "b".repeat(81);
        fs::write(root.join("allow-long.txt"), format!("{long_b}\n")).unwrap();
        fs::write(root.join("x/a.json"), r#"{"cwd": "/Users/x/p"}"#).unwrap();
        // 문자열 안의 JSON(이스케이프된 /)까지 푼다
        fs::write(
            root.join("x/b.jsonl"),
            "{\"data\": \"{\\\"p\\\": \\\"\\\\/home\\\\/y\\\"}\"}\n",
        )
        .unwrap();
        fs::write(
            root.join("x/c.sql"),
            "INSERT INTO t VALUES ('task-1', 'me@example.com', 'ccusage@20.0.25');",
        )
        .unwrap();
        fs::write(root.join("x/d.txt"), format!("{long_a}\n{long_b}\n")).unwrap();
        let frame = |s: &str| {
            ruzstd::encoding::compress_to_vec(
                s.as_bytes(),
                ruzstd::encoding::CompressionLevel::Fastest,
            )
        };
        let zst = [frame("{\"k\": \"ok\"}\n"), frame("{\"k\": \"sk-abc\"}\n")].concat();
        fs::write(root.join("x/e.jsonl.zst"), zst).unwrap();
        fs::write(root.join("x/f.zst"), b"not zstd").unwrap();
        fs::write(
            root.join("x/expected.json"),
            format!(r#"{{"differs": "{long_a}"}}"#),
        )
        .unwrap();
        fs::write(
            root.join("x/g.json"),
            r#"{"model": "grok-4", "risk-free": "task-2"}"#,
        )
        .unwrap();
        let got = personal_data(&root);
        let _ = fs::remove_dir_all(&root);
        assert_eq!(
            got,
            [
                "x/a.json: /Users/".to_string(),
                "x/b.jsonl: /home/".into(),
                "x/c.sql: email address".into(),
                format!("x/d.txt: string over 80 chars (81): {}…", "a".repeat(24)),
                "x/e.jsonl.zst: sk-".into(),
                "x/f.zst: unreadable .zst".into(),
            ]
        );
    }

    #[test]
    fn compare_checks_listed_fields_models_and_step_count() {
        let got = [
            json!({"calls": 1, "output": 5, "by_model": {"m": {"output": 5, "input": 1}}, "project": "a/b"}),
        ];
        let ok = json!({"source": "x", "steps": [{"output": 5, "by_model": {"m": {"output": 5}}}]});
        assert_eq!(compare(&ok, &got), Ok(()));
        let typo = json!({"steps": [{"outptu": 5}]});
        assert!(compare(&typo, &got)
            .unwrap_err()
            .contains("unknown field outptu"));
        let extra_model = json!({"steps": [{"by_model": {"m": {"output": 5}, "n": {}}}]});
        assert!(compare(&extra_model, &got).is_err());
        let cost = [json!({"cost_usd": 0.1 + 0.2})];
        assert_eq!(compare(&json!({"steps": [{"cost_usd": 0.3}]}), &cost), Ok(()), "1e-9 오차");
        assert!(compare(&json!({"steps": [{"cost_usd": 0.31}]}), &cost).is_err());
        let two = json!({"steps": [{}, {}]});
        assert!(compare(&two, &got).unwrap_err().contains("2 steps, got 1"));
    }

    #[test]
    fn env_names_cover_dollar_vars_and_probe_lists() {
        let yaml = "roots: [\"${QWEN_HOME:-~/.qwen}\", \"$CODEX/x\", \"{vendor}\"]\nenv: [ANTHROPIC_API_KEY]\n# API 설명";
        assert_eq!(env_names(yaml), ["QWEN_HOME", "CODEX", "ANTHROPIC_API_KEY"]);
    }
}
