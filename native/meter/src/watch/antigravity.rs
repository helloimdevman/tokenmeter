//! Antigravity 대화 DB 디코더(F12). `conversations/<uuid>.db`의 `gen_metadata.data`
//! (protobuf `CortexStepGeneratorMetadata`)를 사용 시도마다 정규화한 JSON 하나로 푼다.
//! 필드 번호와 모델 이름 표는 ccusage `rust/adapters/antigravity/src/parser.rs`
//! (2d8dea8, MIT © 2025 ryoppippi)를 따른다. 프롬프트(chat_model #1·#2)는 풀지 않는다.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use rusqlite::Connection;
use serde_json::{json, Value};

const DEFAULT_MODEL: &str = "gemini-internal-model";

/// 필드 값. 고정 폭(wire 1·5)은 쓰는 곳이 없어 건너뛴다.
enum Wire<'a> {
    Int(u64),
    Bytes(&'a [u8]),
}

/// 메시지 하나의 필드 목록(번호, 값).
struct Msg<'a>(Vec<(u64, Wire<'a>)>);

fn varint(b: &mut &[u8]) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..10 {
        let (&byte, rest) = b.split_first()?;
        *b = rest;
        v |= u64::from(byte & 0x7f) << (7 * i);
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

impl<'a> Msg<'a> {
    /// 잘렸거나 모르는 wire 형식(3·4·6·7), 필드 번호 0이면 None: 그 메시지를 버린다.
    fn parse(mut b: &'a [u8]) -> Option<Self> {
        let mut out = Vec::new();
        while !b.is_empty() {
            let tag = varint(&mut b)?;
            let n = tag >> 3;
            if n == 0 {
                return None;
            }
            match tag & 7 {
                0 => out.push((n, Wire::Int(varint(&mut b)?))),
                2 => {
                    let len = usize::try_from(varint(&mut b)?).ok()?;
                    let v = b.get(..len)?;
                    b = &b[len..];
                    out.push((n, Wire::Bytes(v)));
                }
                1 => b = b.get(8..)?,
                5 => b = b.get(4..)?,
                _ => return None,
            }
        }
        Some(Msg(out))
    }

    /// 스칼라는 마지막 값이 이긴다.
    fn int(&self, n: u64) -> Option<u64> {
        self.0.iter().rev().find_map(|(k, v)| match v {
            Wire::Int(x) if *k == n => Some(*x),
            _ => None,
        })
    }

    /// 반복 필드(메시지)는 모두, 순서대로.
    fn all(&self, n: u64) -> impl Iterator<Item = &'a [u8]> + '_ {
        self.0.iter().filter_map(move |(k, v)| match v {
            Wire::Bytes(x) if *k == n => Some(*x),
            _ => None,
        })
    }

    /// 메시지는 첫 값.
    fn msg(&self, n: u64) -> Option<&'a [u8]> {
        self.all(n).next()
    }

    /// 문자열은 마지막 값. 공백뿐이면 없음.
    fn text(&self, n: u64) -> Option<String> {
        let s = std::str::from_utf8(self.all(n).last()?).ok()?.trim();
        (!s.is_empty()).then(|| s.to_string())
    }
}

/// `ModelUsageStats` 하나. #1은 모델 enum이지 토큰이 아니다(tokscale·TokenTracker는 input에 더한다).
struct Usage {
    model: Option<u64>,
    input: u64,
    cache_read: u64,
    cache_write: u64,
    output: u64,
    id: Option<String>,
}

/// 시도 id: response(#11) > provider(#12) > message(#7).
fn ident(u: &Msg) -> Option<String> {
    u.text(11)
        .or_else(|| u.text(12).map(|v| format!("provider:{v}")))
        .or_else(|| u.text(7).map(|v| format!("message:{v}")))
}

fn usage(b: &[u8]) -> Option<Usage> {
    let u = Msg::parse(b)?;
    let n = |k| u.int(k).unwrap_or(0);
    Some(Usage {
        model: u.int(1).filter(|&v| v != 0),
        input: n(2),
        cache_write: n(4),
        cache_read: n(5),
        // #3 = 전체 출력, #9 thinking + #10 응답. 추론은 output 안에 든다.
        output: n(3).max(n(9).saturating_add(n(10))),
        id: ident(&u),
    })
}

/// `Timestamp {#1 초, #2 나노}` → 유닉스 밀리초. 초가 0 이하면 없음.
fn stamp(b: &[u8]) -> Option<i64> {
    let t = Msg::parse(b)?;
    let s = i64::try_from(t.int(1)?).ok().filter(|&s| s > 0)?;
    let nanos = t.int(2).unwrap_or(0).min(999_999_999) as i64;
    Some(s.saturating_mul(1000).saturating_add(nanos / 1_000_000))
}

/// 대화 DB 하나를 읽어 사용 시도마다 레코드 하나:
/// `{conversation, response_id, attempt, model, input, cache_read, cache_write, output, timestamp}`.
/// `attempt`는 0 = 마지막 시도(`chat_model.usage`), i = `retry_infos[i-1]`.
/// `response_id`가 없으면 `provider:`·`message:` id, 그것도 없으면 `row:<대화>:<idx>`.
/// `timestamp`(밀리초)는 생성 행 → 같은 id의 스텝 → 대화 시작 순이고, 다 없으면 뺀다.
/// 표가 없거나 행이 깨졌으면 그 부분만 건너뛴다(살아 있는 미터라 DB 전체를 실패시키지 않는다).
pub fn records(conn: &Connection) -> Vec<Value> {
    let conversation = conn
        .path()
        .and_then(|p| Path::new(p).file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let rows = |sql: &str| -> Vec<(i64, Vec<u8>)> {
        let Ok(mut st) = conn.prepare(sql) else {
            return Vec::new();
        };
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .map(|it| it.filter_map(Result::ok).collect())
            .unwrap_or_default()
    };

    // 시도 id → 스텝 시각(#8 completed_at, 없으면 #1 created_at). agy ≥1.1.18은 생성 행 시각을 쓰지 않는다.
    let mut step_ts: HashMap<String, i64> = HashMap::new();
    for (_, b) in rows("SELECT idx, metadata FROM steps WHERE metadata IS NOT NULL") {
        let Some(m) = Msg::parse(&b) else { continue };
        let Some(ts) = m.msg(8).or_else(|| m.msg(1)).and_then(stamp) else {
            continue;
        };
        let retries = m.all(28).filter_map(|r| Msg::parse(r)?.msg(2));
        for u in m.msg(9).into_iter().chain(retries) {
            if let Some(id) = Msg::parse(u).as_ref().and_then(ident) {
                let t = step_ts.entry(id).or_insert(ts);
                *t = (*t).min(ts);
            }
        }
    }
    let started = rows("SELECT rowid, data FROM trajectory_metadata_blob ORDER BY rowid")
        .into_iter()
        .find_map(|(_, b)| Msg::parse(&b)?.msg(2).and_then(stamp));

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut carried: Option<String> = None;
    for (idx, b) in rows("SELECT idx, data FROM gen_metadata ORDER BY idx") {
        let Some(chat) = Msg::parse(&b).and_then(|root| Msg::parse(root.msg(1)?)) else {
            continue;
        };
        let retries = chat.all(17).filter_map(|r| Msg::parse(r)?.msg(2));
        let attempts: Vec<(usize, Usage)> = chat
            .msg(4)
            .into_iter()
            .map(|u| (0, u))
            .chain(retries.enumerate().map(|(i, u)| (i + 1, u)))
            .filter_map(|(n, u)| Some((n, usage(u)?)))
            .collect();
        // 행 모델: #19·#21 이름 → #3 enum → 마지막 시도의 enum. 모델 없는 이어지는 행은 앞 행 모델을 쓴다.
        let row_model = chat
            .text(19)
            .or_else(|| chat.text(21))
            .or_else(|| chat.int(3).filter(|&v| v != 0).map(enum_name))
            .or_else(|| {
                attempts
                    .iter()
                    .find(|(n, _)| *n == 0)?
                    .1
                    .model
                    .map(enum_name)
            });
        if let Some(m) = row_model {
            carried = Some(normalize(&m));
        }
        let ts = chat
            .msg(9)
            .and_then(|s| Msg::parse(s)?.msg(4))
            .and_then(stamp);
        for (attempt, u) in attempts {
            // 0인 시도는 내지 않는다(B1: 0 사본이 키를 먼저 태우지 않게).
            if [u.input, u.cache_read, u.cache_write, u.output] == [0; 4] {
                continue;
            }
            // ponytail: retry_infos가 마지막 시도를 다시 담으면 같은 DB 안에서 먼저 본 것만 센다.
            // 두 사본이 다른 폴에 나뉘어 나타나면 두 번 셀 수 있다. 실데이터로 확인되면 id만으로 키를 잡는다.
            if u.id.as_ref().is_some_and(|id| !seen.insert(id.clone())) {
                continue;
            }
            let id = u.id.unwrap_or_else(|| format!("row:{conversation}:{idx}"));
            let model = u
                .model
                .map(|m| normalize(&enum_name(m)))
                .or_else(|| carried.clone())
                .unwrap_or_else(|| DEFAULT_MODEL.to_string());
            let mut rec = json!({
                "conversation": conversation, "response_id": id, "attempt": attempt, "model": model,
                "input": u.input, "cache_read": u.cache_read, "cache_write": u.cache_write, "output": u.output,
            });
            if let Some(t) = ts.or_else(|| step_ts.get(&id).copied()).or(started) {
                rec["timestamp"] = t.into();
            }
            out.push(rec);
        }
    }
    out
}

/// `Model` enum → 이름(ccusage `model_name_from_id`).
fn enum_name(id: u64) -> String {
    let s = match id {
        246 => "gemini-2.5-pro",
        312 => "gemini-2.5-flash",
        313 | 329 => "gemini-2.5-flash-thinking",
        330 => "gemini-2.5-flash-lite",
        281 | 282 => "claude-4-sonnet",
        290 | 291 => "claude-4-opus",
        333 | 334 => "claude-4.5-sonnet",
        340 | 341 => "claude-4.5-haiku",
        342 => "model_openai_gpt_oss_120b_medium",
        1318 => "gemini-3.8-flash-high",
        1319 => "gemini-3.8-flash-medium",
        1320 => "gemini-3.8-flash-low",
        1298 => "gemini-3.7-flash-high",
        1299 => "gemini-3.7-flash-medium",
        1300 => "gemini-3.7-flash-low",
        1071 => "gemini-3.6-flash-high",
        1072 => "gemini-3.6-flash-medium",
        1073 => "gemini-3.6-flash-low",
        1000.. => return format!("model_placeholder_m{}", id - 1000),
        _ => return format!("antigravity-model-{id}"),
    };
    s.to_string()
}

/// 표시 이름·자리표시 이름 → 모델 id(ccusage `normalize_antigravity_model`).
fn normalize(raw: &str) -> String {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    for v in ["3.8", "3.7", "3.6"] {
        for e in ["high", "medium", "low"] {
            if lower == format!("gemini {v} flash ({e})") {
                return format!("gemini-{v}-flash-{e}");
            }
        }
    }
    let base = lower
        .find('(')
        .map_or(lower.as_str(), |i| lower[..i].trim());
    let s = match base {
        "gemini 3.8 flash" | "gemini 3.8 flash thinking" => "gemini-3.8-flash",
        "gemini 3.7 flash" | "gemini 3.7 flash thinking" => "gemini-3.7-flash",
        "gemini 3.7 pro" | "gemini 3.7 pro thinking" => "gemini-3.7-pro",
        "gemini 3.6 flash" | "gemini 3 flash" => "gemini-3.6-flash",
        "gemini 3.6 pro" => "gemini-3.6-pro",
        "gemini 3 pro" | "gemini 3 pro thinking" => "gemini-3-pro",
        "gemini 2.5 flash" => "gemini-2.5-flash",
        "gemini 2.5 pro" => "gemini-2.5-pro",
        "gemini 2.0 flash" | "gemini 2 flash" => "gemini-2.0-flash",
        "gemini 2.0 pro" => "gemini-2.0-pro",
        "gemini 1.5 flash" => "gemini-1.5-flash",
        "gemini 1.5 pro" => "gemini-1.5-pro",
        "model_placeholder_m318" => "gemini-3.8-flash-high",
        "model_placeholder_m319" => "gemini-3.8-flash-medium",
        "model_placeholder_m320" => "gemini-3.8-flash-low",
        "model_placeholder_m298" => "gemini-3.7-flash-high",
        "model_placeholder_m299" => "gemini-3.7-flash-medium",
        "model_placeholder_m300" => "gemini-3.7-flash-low",
        "model_placeholder_m71" => "gemini-3.6-flash-high",
        "model_placeholder_m72" => "gemini-3.6-flash-medium",
        "model_placeholder_m73" => "gemini-3.6-flash-low",
        "model_placeholder_m26" => "claude-opus-4-6",
        "model_placeholder_m35" => "claude-sonnet-4-6",
        "model_placeholder_m36" | "model_placeholder_m37" | "model_placeholder_m16" => {
            "gemini-3.1-pro"
        }
        "model_placeholder_m18" | "model_placeholder_m84" | "model_placeholder_m47" => {
            "gemini-3-flash-preview"
        }
        "model_placeholder_m132" | "model_placeholder_m133" => "gemini-3.5-flash-high",
        "model_placeholder_m187" => "gemini-3.5-flash-extra-low",
        "model_placeholder_m20" => "gemini-3.5-flash-medium",
        "model_openai_gpt_oss_120b_medium" => "gpt-oss-120b-medium",
        "gemini-pro-default" | "gemini-pro-agent" => "gemini-3.1-pro",
        "gemini-3-flash-agent"
        | "gemini-3-flash-agent-a"
        | "gemini-3-flash-agent-b"
        | "gemini-3-flash-a"
        | "gemini-3-flash-b" => "gemini-3.5-flash-high",
        "gemini-3-flash-c" | "gemini-3-flash" => "gemini-3-flash-preview",
        "gemini-3.5-flash-low" => "gemini-3.5-flash-medium",
        "gemini-3.1-pro-high" | "gemini-3.1-pro-low" => "gemini-3.1-pro",
        "gemini-3-pro-high" | "gemini-3-pro-low" => "gemini-3-pro",
        "claude 3.7 sonnet" | "claude 3.7 sonnet thinking" => "claude-3-7-sonnet",
        "claude 3.5 sonnet" => "claude-3-5-sonnet",
        "claude 3.5 haiku" => "claude-3-5-haiku",
        "claude 3 opus" => "claude-3-opus",
        _ => {
            let c = base.replace(' ', "-");
            let known = ["gemini-", "claude-", "gpt-"]
                .iter()
                .any(|p| c.starts_with(p));
            return if known { c } else { trimmed.to_string() };
        }
    };
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_varint(mut x: u64, o: &mut Vec<u8>) {
        while x >= 0x80 {
            o.push((x as u8 & 0x7f) | 0x80);
            x >>= 7;
        }
        o.push(x as u8);
    }
    fn int(n: u64, x: u64, o: &mut Vec<u8>) {
        put_varint(n << 3, o);
        put_varint(x, o);
    }
    fn bytes(n: u64, x: &[u8], o: &mut Vec<u8>) {
        put_varint((n << 3) | 2, o);
        put_varint(x.len() as u64, o);
        o.extend_from_slice(x);
    }
    fn msg(build: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
        let mut o = Vec::new();
        build(&mut o);
        o
    }
    /// ModelUsageStats: 토큰 칸 (번호, 값)과 id 칸 (번호, 문자열).
    fn usage_blob(tokens: &[(u64, u64)], ids: &[(u64, &str)]) -> Vec<u8> {
        msg(|o| {
            tokens.iter().for_each(|&(n, x)| int(n, x, o));
            ids.iter().for_each(|&(n, s)| bytes(n, s.as_bytes(), o));
        })
    }
    fn ts_blob(s: u64, ns: u64) -> Vec<u8> {
        msg(|o| {
            int(1, s, o);
            int(2, ns, o);
        })
    }
    /// ccusage `metadata_blob`과 같은 모양: chat_model {#3 enum, #4 usage, #9 {#4 시각}, #17 재시도, #19 이름}.
    fn gen_blob(
        model: Option<&str>,
        enum_id: Option<u64>,
        usage: Option<&[u8]>,
        ts: Option<(u64, u64)>,
        retries: &[Vec<u8>],
    ) -> Vec<u8> {
        let chat = msg(|o| {
            if let Some(e) = enum_id {
                int(3, e, o);
            }
            if let Some(u) = usage {
                bytes(4, u, o);
            }
            if let Some((s, ns)) = ts {
                bytes(9, &msg(|g| bytes(4, &ts_blob(s, ns), g)), o);
            }
            for r in retries {
                bytes(17, &msg(|ri| bytes(2, r, ri)), o);
            }
            if let Some(m) = model {
                bytes(19, m.as_bytes(), o);
            }
        });
        msg(|o| bytes(1, &chat, o))
    }
    fn open(path: &str) -> Connection {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "CREATE TABLE gen_metadata (idx INTEGER PRIMARY KEY, data BLOB NOT NULL);
             CREATE TABLE steps (idx INTEGER PRIMARY KEY, metadata BLOB);
             CREATE TABLE trajectory_metadata_blob (data BLOB);",
        )
        .unwrap();
        c
    }
    fn db(gen: &[(i64, Vec<u8>)], steps: &[(i64, Vec<u8>)], traj: &[Vec<u8>]) -> Connection {
        let c = open(":memory:");
        fill(&c, gen, steps, traj);
        c
    }
    fn fill(c: &Connection, gen: &[(i64, Vec<u8>)], steps: &[(i64, Vec<u8>)], traj: &[Vec<u8>]) {
        for (i, d) in gen {
            c.execute(
                "INSERT INTO gen_metadata VALUES (?1, ?2)",
                rusqlite::params![i, d],
            )
            .unwrap();
        }
        for (i, d) in steps {
            c.execute("INSERT INTO steps VALUES (?1, ?2)", rusqlite::params![i, d])
                .unwrap();
        }
        for d in traj {
            c.execute("INSERT INTO trajectory_metadata_blob VALUES (?1)", [d])
                .unwrap();
        }
    }
    fn nums(r: &Value) -> [u64; 4] {
        ["input", "cache_read", "cache_write", "output"].map(|k| r[k].as_u64().unwrap())
    }

    #[test]
    fn one_conversation_with_two_attempts() {
        // 조사 문서 §3.4의 합성 표본: 마지막 시도 + retry_infos 하나.
        let final_usage = usage_blob(
            &[
                (1, 1132),
                (2, 812),
                (3, 340),
                (4, 0),
                (5, 16000),
                (6, 24),
                (9, 300),
                (10, 40),
            ],
            &[(7, "msg-a"), (11, "resp-a")],
        );
        let retry = usage_blob(
            &[(1, 1132), (2, 812), (3, 120), (9, 100), (10, 20)],
            &[(11, "resp-b")],
        );
        let chat = msg(|o| {
            bytes(1, b"system prompt SECRET", o); // 프롬프트: protobuf로 풀면 깨지는 바이트
            bytes(2, b"user message SECRET", o);
            int(3, 1132, o);
            bytes(4, &final_usage, o);
            let start = msg(|g| {
                int(2, u64::MAX, g); // checkpoint_index -1: 10바이트 varint
                bytes(4, &ts_blob(1_789_200_000, 250_000_000), g);
            });
            bytes(9, &start, o);
            bytes(
                17,
                &msg(|ri| {
                    bytes(2, &retry, ri);
                    int(5, 1, ri);
                }),
                o,
            );
            bytes(19, b"gemini-3-flash-a", o);
            bytes(21, b"Gemini 3.5 Flash (High)", o);
        });
        let dir = std::env::temp_dir().join(format!("tm-antigravity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0b7c7f0e-1111-4222-8333-944455556666.db");
        let c = open(path.to_str().unwrap());
        fill(&c, &[(3, msg(|o| bytes(1, &chat, o)))], &[], &[]);

        let recs = records(&c);
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(
            recs[0],
            json!({"conversation": "0b7c7f0e-1111-4222-8333-944455556666", "response_id": "resp-a",
                   "attempt": 0, "model": "gemini-3.5-flash-high", "input": 812, "cache_read": 16000,
                   "cache_write": 0, "output": 340, "timestamp": 1_789_200_000_250i64})
        );
        assert_eq!(recs[1]["response_id"], "resp-b");
        assert_eq!(recs[1]["attempt"], 1);
        assert_eq!(nums(&recs[1]), [812, 0, 0, 120]);
        assert!(!serde_json::to_string(&recs).unwrap().contains("SECRET"));
    }

    #[test]
    fn unknown_fields_are_skipped() {
        let plain = usage_blob(&[(2, 5), (3, 7)], &[(11, "r")]);
        let noisy = msg(|o| {
            o.extend_from_slice(&plain);
            put_varint((30 << 3) | 5, o); // fixed32
            o.extend_from_slice(&[1, 2, 3, 4]);
            put_varint((31 << 3) | 1, o); // fixed64
            o.extend_from_slice(&[0; 8]);
            bytes(32, b"\xff\xfe", o);
            int(33, 99, o);
        });
        let root = msg(|o| {
            o.extend_from_slice(&gen_blob(None, None, Some(&noisy), None, &[]));
            bytes(2, &[6, 7], o); // step_indices
            bytes(99, b"x", o);
        });
        let a = records(&db(
            &[(1, gen_blob(None, None, Some(&plain), None, &[]))],
            &[],
            &[],
        ));
        let b = records(&db(&[(1, root)], &[], &[]));
        assert_eq!(a.len(), 1);
        assert_eq!(a, b);
    }

    #[test]
    fn truncated_bytes_drop_only_that_row() {
        let u1 = usage_blob(&[(2, 10)], &[(11, "r1")]);
        let u2 = usage_blob(&[(2, 20)], &[(11, "r2")]);
        let whole = gen_blob(None, None, Some(&u2), None, &[]);
        let cut = whole[..whole.len() - 2].to_vec();
        let bad_wire = msg(|o| put_varint((7 << 3) | 3, o)); // 그룹 시작(wire 3)은 받지 않는다
        let c = db(
            &[
                (1, cut),
                (2, gen_blob(None, None, Some(&u1), None, &[])),
                (3, bad_wire),
            ],
            &[],
            &[],
        );
        let recs = records(&c);
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["response_id"], "r1");
    }

    #[test]
    fn zero_attempts_and_repeated_final_are_not_emitted() {
        let fin = usage_blob(&[(2, 10), (3, 4)], &[(11, "r-final")]);
        let zero = usage_blob(&[], &[(11, "r-zero")]);
        let c = db(
            &[(
                1,
                gen_blob(None, None, Some(&fin), None, &[zero, fin.clone()]),
            )],
            &[],
            &[],
        );
        let recs = records(&c);
        assert_eq!(recs.len(), 1);
        assert_eq!(
            (recs[0]["response_id"].as_str(), recs[0]["attempt"].as_u64()),
            (Some("r-final"), Some(0))
        );
    }

    #[test]
    fn ids_and_timestamps_fall_back() {
        // 1: 생성 행 시각 없음 → 같은 response id 스텝의 #8. 2: 스텝도 없음 → 대화 시작.
        // 3: id가 하나도 없음 → provider·message 없이 row id, 재시도는 attempt로 갈린다.
        let s = usage_blob(&[(2, 1)], &[(11, "r-step")]);
        let t = usage_blob(&[(2, 2)], &[(7, "m-only")]);
        let bare = usage_blob(&[(2, 3)], &[]);
        let step = msg(|o| {
            bytes(1, &ts_blob(1_700_000_000, 0), o);
            bytes(8, &ts_blob(1_700_000_050, 500_000_000), o);
            bytes(9, &usage_blob(&[(2, 1)], &[(11, "r-step")]), o);
        });
        let traj = msg(|o| bytes(2, &ts_blob(1_690_000_000, 0), o));
        let c = db(
            &[
                (1, gen_blob(None, None, Some(&s), None, &[])),
                (2, gen_blob(None, None, Some(&t), None, &[])),
                (
                    3,
                    gen_blob(None, None, Some(&bare), None, std::slice::from_ref(&bare)),
                ),
            ],
            &[(0, step)],
            &[traj],
        );
        let recs = records(&c);
        let got: Vec<_> = recs
            .iter()
            .map(|r| {
                (
                    r["response_id"].as_str().unwrap(),
                    r["attempt"].as_u64().unwrap(),
                    r["timestamp"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("r-step", 0, 1_700_000_050_500),
                ("message:m-only", 0, 1_690_000_000_000),
                ("row::3", 0, 1_690_000_000_000),
                ("row::3", 1, 1_690_000_000_000),
            ]
        );
        assert!(recs.iter().all(|r| r["model"] == DEFAULT_MODEL));
    }

    #[test]
    fn missing_tables_or_rows_are_empty() {
        assert!(records(&Connection::open_in_memory().unwrap()).is_empty());
        assert!(records(&db(&[], &[], &[])).is_empty());
    }

    // source: ccusage 2d8dea8 rust/adapters/antigravity/src/loader.rs:150-219
    // `loads_real_schema_rows_with_continuations_and_token_buckets`(MIT © 2025 ryoppippi).
    // 단언값: #1 in 400, cw 30, cr 500, 출력 150 + 추론 50, "Gemini 3 Pro" → gemini-3-pro.
    // #2 in 300, 출력 75 + 추론 25, 모델은 앞 행에서 이어 받음. 우리 output은 추론을 포함한 합.
    #[test]
    fn ccusage_continuations_and_token_buckets() {
        let first = usage_blob(
            &[(2, 400), (3, 200), (4, 30), (5, 500), (9, 50), (10, 150)],
            &[(7, "message-1"), (11, "response-1"), (12, "provider-1")],
        );
        let cont = usage_blob(
            &[(2, 300), (3, 100), (9, 25), (10, 75)],
            &[(7, "message-2"), (11, "response-2")],
        );
        let c = db(
            &[
                (
                    2,
                    gen_blob(None, None, Some(&cont), Some((1_778_000_001, 0)), &[]),
                ),
                (
                    1,
                    gen_blob(
                        Some("Gemini 3 Pro"),
                        None,
                        Some(&first),
                        Some((1_778_000_000, 123_000_000)),
                        &[],
                    ),
                ),
            ],
            &[],
            &[],
        );
        let recs = records(&c);
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0]["response_id"], "response-1");
        assert_eq!(nums(&recs[0]), [400, 500, 30, 150 + 50]);
        assert_eq!(nums(&recs[1]), [300, 0, 0, 75 + 25]);
        assert_eq!(recs[0]["model"], "gemini-3-pro");
        assert_eq!(recs[1]["model"], "gemini-3-pro");
        assert_eq!(recs[0]["timestamp"], 1_778_000_000_123i64);
        assert_eq!(recs[1]["timestamp"], 1_778_000_001_000i64);
    }

    // source: ccusage 2d8dea8 rust/adapters/antigravity/src/loader.rs:221-251
    // `decodes_model_id_without_counting_it_as_input_tokens`: usage.#1 = 246은 모델이고 input은 4.
    #[test]
    fn ccusage_model_id_is_not_input() {
        let u = usage_blob(
            &[(1, 246), (2, 4), (3, 6), (10, 6)],
            &[(11, "model-id-response")],
        );
        let recs = records(&db(
            &[(
                1,
                gen_blob(None, Some(246), Some(&u), Some((1_778_000_000, 0)), &[]),
            )],
            &[],
            &[],
        ));
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["model"], "gemini-2.5-pro");
        assert_eq!(recs[0]["input"], 4);
    }

    #[test]
    fn model_names_follow_ccusage_table() {
        assert_eq!(normalize(&enum_name(1132)), "gemini-3.5-flash-high");
        assert_eq!(normalize(&enum_name(1319)), "gemini-3.8-flash-medium");
        assert_eq!(normalize(&enum_name(290)), "claude-4-opus");
        assert_eq!(normalize(&enum_name(7)), "antigravity-model-7");
        assert_eq!(normalize("Gemini 3.7 Flash (Low)"), "gemini-3.7-flash-low");
        assert_eq!(
            normalize("Claude 3.7 Sonnet (Thinking)"),
            "claude-3-7-sonnet"
        );
        assert_eq!(normalize("Some Label"), "Some Label");
    }
}
