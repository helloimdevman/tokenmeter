//! 레코드 하나의 처리 순서: 문맥, match, 토큰 벡터, 중복 제거, 델타.

use super::cond;
use super::delta::{TokenDelta, Vector};
use super::expr::{Env, Pick};
use super::probe::resolve_endpoint;
use super::reader::{path_key, ServiceReader, SEEN_CAP};
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
fn int(pick: Option<&Pick>, env: &Env) -> i64 {
    pick.and_then(|p| p.num(env)).unwrap_or(0.0).max(0.0) as i64
}

impl ServiceReader {
    pub(super) fn handle(
        &mut self,
        obj: &Value,
        path: &Path,
        line_no: u64,
        out: &mut Vec<TokenDelta>,
        emit: bool,
    ) {
        let key = path_key(path);
        // 문맥 학습: 이 레코드에 값이 있으면 파일 문맥을 바꾼다. 문맥 식의 `$ctx`는 앞 레코드까지 배운 값.
        let ctx_map = {
            let learned = self.ctx.entry(key.clone()).or_default();
            let found: Vec<(String, String)> = {
                let get = |name: &str| learned.get(name).cloned();
                let env = Env::new(obj, &get);
                self.x
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
        if !cond::all(&self.x.conds, &env) {
            return;
        }
        let vector: Vector = std::array::from_fn(|i| int(self.x.fields[i].as_ref(), &env));
        let diff: Vector = if self.spec.mode == "cumulative" {
            let record_key = self
                .x
                .key
                .as_ref()
                .and_then(|k| k.text(&env))
                .unwrap_or_else(|| key.clone());
            let bases = self.base.entry(key.clone()).or_default();
            let previous = bases.get(&record_key).copied();
            bases.insert(record_key, vector);
            match previous {
                None if self.blind.remove(&key) => return,
                None => vector,
                Some(prev) => {
                    let d = [
                        vector[0] - prev[0],
                        vector[1] - prev[1],
                        vector[2] - prev[2],
                        vector[3] - prev[3],
                    ];
                    if d.iter().any(|v| *v < 0) {
                        return;
                    }
                    d
                }
            }
        } else {
            if let Some(k) = &self.x.key {
                if let Some(raw) = k.text(&env) {
                    if !raw.is_empty() && !self.seen_keys.insert(raw) {
                        return;
                    }
                    if self.seen_keys.len() > SEEN_CAP {
                        let remove: Vec<String> =
                            self.seen_keys.iter().take(SEEN_CAP / 2).cloned().collect();
                        for key in remove {
                            self.seen_keys.remove(&key);
                        }
                    }
                } else {
                    let seen = self.seen.entry(key.clone()).or_default();
                    let mark = format!("{key}:{line_no}");
                    if !seen.insert(mark) {
                        return;
                    }
                }
            } else {
                let seen = self.seen.entry(key.clone()).or_default();
                let mark = format!("{key}:{line_no}");
                if !seen.insert(mark) {
                    return;
                }
            }
            vector
        };
        let mut input_tokens = diff[0];
        let cache_read = diff[1];
        let cache_write = diff[2];
        let mut output_tokens = diff[3];
        if self.spec.input_includes_cache {
            input_tokens = (input_tokens - cache_read).max(0);
        }
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
        let endpoint = self.endpoint_for(&session, &vendor);
        output_tokens = self.adjust_live_output(&env, &session, output_tokens);
        let duration_ms = int(self.x.duration_ms.as_ref(), &env);
        let subagent = self.x.subagent.as_ref().is_some_and(|p| p.truthy(&env));
        let (ctx_now, ctx_win) = if subagent {
            (0, 0)
        } else if let Some(p) = &self.x.ctx_tokens {
            let current = int(Some(p), &env);
            let window = Some(int(self.x.ctx_window.as_ref(), &env))
                .filter(|n| *n > 0)
                .unwrap_or_else(|| crate::pricing::context_window(&model, current));
            (current, window)
        } else {
            let current = vector[0] + vector[1] + vector[2];
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
            plan: self.plan.clone(),
            endpoint,
            cwd,
            effort,
            ctx_tokens: ctx_now,
            ctx_window: ctx_win,
            subagent,
            duration_ms,
            ..Default::default()
        };
        if delta.total() <= 0 {
            return;
        }
        if emit {
            out.push(std::mem::take(&mut delta));
        }
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
        let value = resolve_endpoint(
            &self.spec,
            self.x.endpoint_key.as_ref(),
            env.as_ref(),
            vendor,
            &self.plan,
        );
        if self.endpoint.len() > 1000 {
            self.endpoint.clear();
        }
        self.endpoint.insert(key, value.clone());
        value
    }

    fn adjust_live_output(&mut self, env: &Env, session: &str, output_tokens: i64) -> i64 {
        let Some(text) = &self.x.live_chars else {
            return output_tokens;
        };
        let extra = text
            .text(env)
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
