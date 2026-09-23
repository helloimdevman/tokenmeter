//! 출력 토큰 처리량 EMA. Python `LiveRate` 와 같은 시정수.

use serde_json::Value;
use std::collections::HashMap;

pub const RATE_TAU: f64 = 20.0;
pub const RATE_FLOOR: f64 = 0.01;

fn as_i64(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0) as i64,
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}

fn span(state: &Value) -> f64 {
    let sec = state.get("out_sec").and_then(Value::as_f64).unwrap_or(0.0);
    if !sec.is_finite() || sec <= RATE_TAU {
        RATE_TAU
    } else {
        sec
    }
}

#[derive(Default)]
pub struct LiveRate {
    pub rate: f64,
    pub rates: HashMap<String, f64>,
    last_output: Option<i64>,
    seen_out: HashMap<String, i64>,
}

impl LiveRate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, state: &Value) -> i64 {
        self.track_sessions(state);
        let cur = state
            .get("total")
            .and_then(Value::as_object)
            .and_then(|n| n.get("totals"))
            .and_then(Value::as_object)
            .and_then(|t| t.get("output_tokens"))
            .map(|v| as_i64(Some(v)))
            .unwrap_or(0);
        if self.last_output.is_none() {
            self.last_output = Some(cur);
            return 0;
        }
        let prev = self.last_output.unwrap_or(0);
        let gained = (cur - prev).max(0);
        self.last_output = Some(cur);
        if gained > 0 {
            self.rate += gained as f64 / span(state);
        }
        gained
    }

    pub fn tick(&mut self, dt: f64) {
        let decay = (-dt / RATE_TAU).exp();
        self.rate *= decay;
        if self.rate < RATE_FLOOR {
            self.rate = 0.0;
        }
        self.rates.retain(|_, value| {
            *value *= decay;
            *value >= RATE_FLOOR
        });
    }

    fn track_sessions(&mut self, state: &Value) {
        let book = state.get("sessions").and_then(Value::as_object);
        let Some(book) = book else {
            self.seen_out.clear();
            self.rates.clear();
            return;
        };
        let keys: Vec<String> = book.keys().cloned().collect();
        for (key, rec) in book {
            let Some(rec) = rec.as_object() else { continue };
            let totals = rec.get("totals").and_then(Value::as_object);
            let total_out = as_i64(totals.and_then(|t| t.get("output_tokens")));
            let sub = as_i64(rec.get("sub_output_tokens"));
            let cur = (total_out - sub).max(0);
            let prev = self.seen_out.get(key).copied();
            self.seen_out.insert(key.clone(), cur);
            if let Some(prev) = prev {
                if cur > prev {
                    let add = (cur - prev) as f64 / span(state);
                    *self.rates.entry(key.clone()).or_insert(0.0) += add;
                }
            }
        }
        self.seen_out.retain(|k, _| keys.contains(k));
        self.rates.retain(|k, _| keys.contains(k));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn total(output: i64) -> Value {
        json!({"total": {"totals": {"output_tokens": output}}})
    }

    #[test]
    fn bursts_long_turns_idle_decay_and_main_session_rate() {
        let mut live = LiveRate::new();
        live.update(&total(40_000));
        assert_eq!(live.rate, 0.0, "첫 관측은 기준점이다");
        live.update(&total(41_500));
        assert_eq!(live.rate, 1500.0 / RATE_TAU);
        assert!(live.rate < 100.0, "한 턴이 통째로 와도 1500 tok/s 로 튀면 안 된다");

        let mut long = LiveRate::new();
        long.update(&json!({"total": {"totals": {"output_tokens": 0}}, "out_sec": 0}));
        long.update(&json!({"total": {"totals": {"output_tokens": 24704}}, "out_sec": 402.847}));
        assert!((long.rate - 24704.0 / 402.847).abs() < 0.5, "{}", long.rate);

        let mut idle = LiveRate::new();
        idle.update(&total(0));
        idle.update(&total((600.0 * RATE_TAU) as i64));
        assert_eq!(idle.rate, 600.0);
        idle.tick(10.0);
        assert!(idle.rate > 0.0 && idle.rate < 600.0);
        idle.tick(300.0);
        assert_eq!(idle.rate, 0.0);

        let mut rec = json!({"totals": {"output_tokens": 400}, "sub_output_tokens": 300});
        let mut session = LiveRate::new();
        session.update(&json!({"sessions": {"svc/mixed": rec.clone()}}));
        assert!(session.rates.is_empty());
        rec["totals"]["output_tokens"] = json!(420);
        rec["sub_output_tokens"] = json!(320);
        session.update(&json!({"sessions": {"svc/mixed": rec.clone()}}));
        assert_eq!(session.rates.get("svc/mixed").copied().unwrap_or(0.0), 0.0, "서브 증가는 세션 속도가 아니다");
        rec["totals"]["output_tokens"] = json!(470);
        session.update(&json!({"sessions": {"svc/mixed": rec}}));
        assert_eq!(session.rates["svc/mixed"], 50.0 / RATE_TAU);
    }
}
