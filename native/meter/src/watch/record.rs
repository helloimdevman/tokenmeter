//! 레코드 하나의 처리 순서: 문맥, match, 필드, 토큰 의미(F5), 키와 모드(F2), 델타.

use super::cond;
use super::delta::TokenDelta;
use super::expr::{Env, Pick};
use super::ledger::{self, key_hash, record_hash, Vals};
use super::probe::{resolve_endpoint, resolve_plan};
use super::reader::{path_key, ServiceReader, Source};
use super::time::parse_ts;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;
use tokenmeter_hook::live_path;

/// ponytail: Grok 프롬프트 경로가 박혀 있다. 2.12가 `live_chars.turn` 식으로 연다.
static LIVE_PROMPT: LazyLock<Pick> = LazyLock::new(|| {
    let paths = serde_yaml::from_str("[params._meta.promptId, params.update.prompt_id]");
    Pick::from_yaml(&paths.expect("상수")).expect("상수")
});

/// 숫자 자리의 최종값. 없으면 0, 0 아래로 내리지 않는다.
fn val(pick: Option<&Pick>, env: &Env) -> f64 {
    pick.and_then(|p| p.num(env)).unwrap_or(0.0).max(0.0)
}

fn int(pick: Option<&Pick>, env: &Env) -> i64 {
    val(pick, env) as i64
}

impl ServiceReader {
    pub(super) fn handle(
        &mut self,
        src: &mut Source,
        obj: &Value,
        path: &Path,
        out: &mut Vec<TokenDelta>,
        emit: bool,
    ) {
        let key = path_key(path);
        // 문맥 학습: 이 레코드에 값이 있으면 파일 문맥을 바꾼다. 문맥 식의 `$ctx`는 앞 레코드까지 배운 값.
        let ctx_map = {
            let learned = src.ctx.entry(key.clone()).or_default();
            let found: Vec<(String, String)> = {
                let get = |name: &str| learned.get(name).cloned();
                let env = Env::new(obj, &get);
                src.x
                    .context
                    .iter()
                    .filter_map(|(name, p)| Some((name.clone(), p.text(&env)?)))
                    .collect()
            };
            learned.extend(found);
            learned.clone()
        };
        let get = |name: &str| ctx_map.get(name).cloned();
        let env = Env::new(obj, &get);
        self.stats.records += 1;
        // 레코드 시각(F8). 파일의 첫 시각은 match와 상관없이 잡는다(4.5).
        let at = src.x.timestamp.as_ref().and_then(|p| p.value(&env)).and_then(|v| parse_ts(&v));
        if let Some(t) = at {
            src.first.entry(key.clone()).or_insert(t);
        }
        if !cond::all(&src.x.conds, &env) {
            self.stats.dropped_by_match += 1;
            return;
        }
        self.stats.matched += 1;
        // 필드와 토큰 의미(F5): input에 든 칸은 그 합이 input 이하일 때만 뺀다.
        let mut vals: Vals = [0.0; 6];
        for (i, (slot, pick)) in vals.iter_mut().zip(&src.x.fields).enumerate() {
            if pick.as_ref().is_some_and(|p| p.num(&env).is_some()) {
                self.stats.hits[i] += 1;
            }
            *slot = val(pick.as_ref(), &env).trunc();
        }
        vals[4] = val(src.x.cost_usd.as_ref(), &env);
        vals[5] = val(src.x.duration_ms.as_ref(), &env);
        let inside: f64 = src.x.input_includes.iter().map(|i| vals[*i]).sum();
        if inside <= vals[0] {
            vals[0] -= inside;
        }
        if let Some(p) = &src.x.rebase_on {
            let h = key_hash(&p.text(&env).unwrap_or_default());
            if src.roll.insert(key.clone(), h).is_some_and(|old| old != h) {
                src.rolling = true;
            }
        }
        // 키와 모드(F2)
        let stream = src
            .x
            .key
            .as_ref()
            .and_then(|k| k.text(&env))
            .map(|k| key_hash(&k));
        let (d, calls, fresh) = if src.spec.mode == "cumulative" {
            let Some((d, calls)) = self.cumulative(src, &key, stream, vals) else {
                return;
            };
            (d, calls, true)
        } else {
            let k = stream.unwrap_or_else(|| record_hash(obj));
            let fresh = !self.ledger.has(k);
            let (d, calls) = self.ledger.grow(k, vals);
            (d, calls, fresh)
        };
        let input_tokens = d[0] as i64;
        let cache_read = d[1] as i64;
        let cache_write = d[2] as i64;
        let mut output_tokens = d[3] as i64;
        // 이 레코드의 값은 위에서 배웠으므로 파일 문맥만 보면 된다.
        let pick_ctx = |name: &str, fallback: &str| {
            ctx_map
                .get(name)
                .cloned()
                .unwrap_or_else(|| fallback.to_string())
        };
        let model = pick_ctx("model", &self.spec.default_model);
        let cwd = pick_ctx("cwd", "");
        let vendor = {
            let v = pick_ctx("vendor", "");
            if v.is_empty() {
                if self.spec.vendor.is_empty() {
                    crate::pricing::vendor_of(&model)
                } else {
                    self.spec.vendor.clone()
                }
            } else {
                v
            }
        };
        let session = pick_ctx("session", "");
        let effort = pick_ctx("effort", "");
        let plan = self.plan_for(&vendor);
        // 레코드의 엔드포인트가 먼저다(스펙 9절). 프로브 결과처럼 사용자 정보·쿼리를 지운다.
        let endpoint = match pick_ctx("endpoint", "") {
            e if e.trim().is_empty() => self.endpoint_for(&session, &vendor),
            e => tokenmeter_hook::normalize_endpoint(&e),
        };
        output_tokens = self.adjust_live_output(&env, &session, output_tokens, fresh);
        let subagent = src.x.subagent.as_ref().is_some_and(|p| p.truthy(&env));
        let (ctx_now, ctx_win) = if subagent {
            (0, 0)
        } else if let Some(p) = &src.x.ctx_tokens {
            let current = int(Some(p), &env);
            let window = Some(int(src.x.ctx_window.as_ref(), &env))
                .filter(|n| *n > 0)
                .unwrap_or_else(|| crate::pricing::context_window(&model, current));
            (current, window)
        } else {
            let current = (vals[0] + vals[1] + vals[2]) as i64;
            (current, crate::pricing::context_window(&model, current))
        };
        // 칸이 늘어도(Task 1.11) 여기를 고치지 않게 기본값으로 끝낸다.
        #[allow(clippy::needless_update)]
        let mut delta = TokenDelta {
            input_tokens,
            cache_read,
            cache_write,
            output_tokens,
            model,
            service: self.spec.name.clone(),
            project: tokenmeter_hook::project_key(&cwd),
            session,
            vendor,
            plan,
            endpoint,
            cwd,
            effort,
            ctx_tokens: ctx_now,
            ctx_window: ctx_win,
            subagent,
            duration_ms: d[5] as i64,
            at: at.unwrap_or(0.0),
            calls: u32::from(calls),
            cost_usd: (d[4] > 0.0).then_some(d[4]),
            ..Default::default()
        };
        // 토큰 차이 합이 0이고 비용 차이도 0일 때만 버린다(F2).
        if delta.total() <= 0 && delta.cost_usd.is_none() {
            return;
        }
        if emit {
            out.push(std::mem::take(&mut delta));
        }
    }

    /// 벤더마다 최종 요금제 라벨만 캐시한다(프로브 문서는 버린다, 스펙 2.0).
    /// ponytail: 프로브 파일이 바뀌어도 리더가 살아 있는 동안은 옛 라벨이다. 필요하면 mtime을 캐시 키에 넣는다.
    pub(super) fn plan_for(&mut self, vendor: &str) -> String {
        if let Some(plan) = self.plan.get(vendor) {
            return plan.clone();
        }
        let plan = resolve_plan(&self.spec, self.x.plan_key.as_ref(), vendor);
        if self.plan.len() > 1000 {
            self.plan.clear();
        }
        self.plan.insert(vendor.to_string(), plan.clone());
        plan
    }

    pub(super) fn endpoint_for(&mut self, session: &str, vendor: &str) -> String {
        let key = format!("{session}|{vendor}");
        if let Some(value) = self.endpoint.get(&key) {
            return value.clone();
        }
        let env = if session.is_empty() {
            None
        } else {
            fs::read_to_string(live_path(&self.spec.name, session))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|value| value.get("routing_env").and_then(Value::as_object).cloned())
                .map(|values| {
                    values
                        .into_iter()
                        .filter_map(|(k, v)| Some((k, v.as_str()?.to_string())))
                        .collect::<HashMap<_, _>>()
                })
        };
        let plan = self.plan_for(vendor);
        let value = resolve_endpoint(&self.spec, self.x.endpoint_key.as_ref(), env.as_ref(), vendor, &plan);
        if self.endpoint.len() > 1000 {
            self.endpoint.clear();
        }
        self.endpoint.insert(key, value.clone());
        value
    }

    /// cumulative(F2): 스트림(키, 없으면 파일)의 기준값과의 차이와 calls. None이면 낼 것이 없다.
    /// `src`는 `with_source`로 꺼내 둔 소스라 `self.sources`에는 다른 소스만 남아 있다.
    fn cumulative(&mut self, src: &mut Source, file: &str, stream: Option<u64>, v: Vals) -> Option<(Vals, bool)> {
        let mine = src.base.get(file).and_then(|b| b.get(&stream)).copied();
        // 이 파일에 기준값이 없으면 같은 서비스 다른 파일 항목(다른 소스 포함)의 같은 키(파일 사이 복사본)
        // ponytail: 파일 수만큼 훑는다(파일·스트림마다 처음 한 번). 느려지면 키 → 파일 색인을 둔다.
        let other = match (mine, stream) {
            (None, Some(_)) => src
                .base
                .iter()
                .filter(|(f, _)| f.as_str() != file)
                .chain(self.sources.iter().flat_map(|s| &s.base))
                .filter_map(|(_, b)| b.get(&stream).copied())
                .max_by(|a, b| a[..4].iter().sum::<f64>().total_cmp(&b[..4].iter().sum())),
            _ => None,
        };
        src.base
            .entry(file.to_string())
            .or_default()
            .insert(stream, v);
        if src.rolling {
            return None;
        }
        if mine.is_none() && other.is_none() && src.blind.remove(file) {
            return None;
        }
        // 기준값이 어디에도 없으면 지금처럼 0에서 시작한 스트림으로 본다(2.9가 문턱으로 가른다).
        let mut base = mine;
        ledger::diff(&mut base, other.or(Some([0.0; 6])), v)
    }

    /// `fresh`가 아니면(이미 본 키) 추정하지 않는다: 다시 읽힌 청크 줄을 두 번 세지 않는다.
    fn adjust_live_output(
        &mut self,
        env: &Env,
        session: &str,
        output_tokens: i64,
        fresh: bool,
    ) -> i64 {
        let Some(text) = &self.x.live_chars else {
            return output_tokens;
        };
        let extra = text
            .text(env)
            .filter(|_| fresh)
            .map(|t| t.chars().count().div_ceil(4).max(1) as i64)
            .unwrap_or(0);
        let prompt = LIVE_PROMPT.text(env).unwrap_or_default();
        let turn = if prompt.is_empty() {
            session.to_string()
        } else {
            format!("{session}|{prompt}")
        };
        if output_tokens > 0 {
            let prev = self.live_out.remove(&turn).unwrap_or(0);
            return (output_tokens - prev).max(0);
        }
        if extra <= 0 {
            return output_tokens;
        }
        *self.live_out.entry(turn).or_insert(0) += extra;
        if self.live_out.len() > 10_000 {
            let remove: Vec<String> = self.live_out.keys().take(5000).cloned().collect();
            for key in remove {
                self.live_out.remove(&key);
            }
        }
        extra
    }
}
