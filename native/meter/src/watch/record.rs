//! 레코드 하나의 처리 순서: 문맥, match, 토큰 벡터, 중복 제거, 델타.

use super::delta::{TokenDelta, Vector};
use super::expr::{dig, dig_string, is_truthy, num};
use super::probe::resolve_endpoint;
use super::reader::{path_key, ServiceReader, SEEN_CAP, TOKEN_FIELDS};
use super::spec::MatchWant;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use tokenmeter_hook::live_path;

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
        let ctx = self.ctx.entry(key.clone()).or_default();
        for (name, dot) in &self.spec.context {
            if let Some(v) = dig_string(obj, dot) {
                if !v.is_empty() {
                    ctx.insert(name.clone(), v);
                }
            }
        }
        if !self.matches(obj) {
            return;
        }
        let vector: Vector = std::array::from_fn(|i| {
            let field = TOKEN_FIELDS[i];
            let path = self.spec.fields.get(field).and_then(|p| p.as_deref());
            num(dig(obj, path.unwrap_or("")))
        });
        let diff: Vector = if self.spec.mode == "cumulative" {
            let record_key = self
                .spec
                .key
                .as_ref()
                .and_then(|k| dig_string(obj, k))
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
            if let Some(k) = &self.spec.key {
                if let Some(raw) = dig_string(obj, k) {
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
        let ctx_map = self.ctx.get(&key).cloned().unwrap_or_default();
        let pick_ctx = |name: &str, fallback: &str| {
            self.spec
                .context
                .get(name)
                .and_then(|dot| dig_string(obj, dot))
                .filter(|s| !s.is_empty())
                .or_else(|| ctx_map.get(name).cloned())
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
        output_tokens = self.adjust_live_output(obj, &session, output_tokens);
        let duration_ms = self
            .spec
            .duration_ms
            .as_ref()
            .map(|p| num(dig(obj, p)))
            .unwrap_or(0);
        let subagent = self
            .spec
            .subagent
            .as_ref()
            .map(|p| dig(obj, p).map(is_truthy).unwrap_or(false))
            .unwrap_or(false);
        let (ctx_now, ctx_win) = if subagent {
            (0, 0)
        } else if let Some(p) = &self.spec.ctx_tokens {
            let current = num(dig(obj, p));
            let window = self
                .spec
                .ctx_window
                .as_ref()
                .map(|w| num(dig(obj, w)))
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

    fn matches(&self, obj: &Value) -> bool {
        for (dot, want) in &self.spec.match_fields {
            let got = dig_string(obj, dot).unwrap_or_else(|| "null".into());
            match want {
                MatchWant::Many(items) => {
                    if !items.iter().any(|w| w == &got) {
                        return false;
                    }
                }
                MatchWant::One(w) => {
                    if &got != w {
                        return false;
                    }
                }
            }
        }
        true
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
        let value = resolve_endpoint(&self.spec, env.as_ref(), vendor, &self.plan);
        if self.endpoint.len() > 1000 {
            self.endpoint.clear();
        }
        self.endpoint.insert(key, value.clone());
        value
    }

    fn adjust_live_output(&mut self, obj: &Value, session: &str, output_tokens: i64) -> i64 {
        let Some(path) = &self.spec.live_chars else {
            return output_tokens;
        };
        let extra = dig_string(obj, path)
            .map(|t| {
                if t.is_empty() {
                    0
                } else {
                    t.chars().count().div_ceil(4).max(1) as i64
                }
            })
            .unwrap_or(0);
        let prompt = dig_string(obj, "params._meta.promptId")
            .or_else(|| dig_string(obj, "params.update.prompt_id"))
            .unwrap_or_default();
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
