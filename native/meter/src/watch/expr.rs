//! 레코드 안의 값 찾기. Task 1.1이 경로식으로 넓힌다.

use serde_json::Value;

pub fn dig<'a>(obj: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return None;
    }
    let mut cur = obj;
    for part in path.split('.') {
        cur = match cur {
            Value::Object(m) => m.get(part)?,
            Value::Array(a) => a.get(part.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

pub(super) fn dig_string(obj: &Value, path: &str) -> Option<String> {
    match dig(obj, path)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

pub(super) fn num(v: Option<&Value>) -> i64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0).max(0.0) as i64,
        Some(Value::String(s)) => s.parse::<f64>().ok().unwrap_or(0.0).max(0.0) as i64,
        _ => 0,
    }
}

pub(super) fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => s != "false" && s != "0" && !s.is_empty(),
        Value::Null => false,
        _ => true,
    }
}
