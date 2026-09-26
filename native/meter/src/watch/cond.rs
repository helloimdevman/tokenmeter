//! match 조건(F7). 항목 사이는 AND. 쓰는 에이전트가 없는 접두·정규식은 넣지 않는다.

use super::expr::{legacy_path, Env, Expr};
use super::spec::yaml_scalar;
use serde_yaml::{Mapping, Value};

#[derive(Clone, Debug)]
pub enum Cond {
    In(Expr, Vec<String>),
    Exists(Expr, bool),
    NotIn(Expr, Vec<String>),
    Any(Vec<Vec<Cond>>),
}

/// `legacy`는 사용자 덮어쓰기: 옛 `a.0.b` 키와 `X: null`(= `{$exists: false}`)을 받는다.
pub fn parse(m: &Mapping, legacy: bool) -> Result<Vec<Cond>, String> {
    let mut out = vec![];
    for (key, want) in m {
        let key = yaml_scalar(key);
        if key == "$any" {
            let groups = want.as_sequence().ok_or_else(|| {
                crate::l10n!(
                    "`$any` takes a list of conditions",
                    "`$any`에는 조건 목록이 와야 합니다"
                )
            })?;
            let groups = groups.iter().map(|g| match g {
                Value::Mapping(g) => parse(g, legacy),
                _ => Err(crate::l10n!(
                    "`$any` items are conditions like {{a: b}}, got {g:?}",
                    "`$any`의 항목은 {{a: b}} 같은 조건이어야 하는데 {g:?}가 왔습니다"
                )),
            });
            out.push(Cond::Any(groups.collect::<Result<_, _>>()?));
            continue;
        }
        let path = if legacy {
            legacy_path(&key)
        } else {
            key.clone()
        };
        let expr = Expr::parse(&path)?;
        match want {
            Value::Null if legacy => out.push(Cond::Exists(expr, false)),
            Value::Mapping(ops) => {
                for (op, arg) in ops {
                    out.push(match (yaml_scalar(op).as_str(), arg) {
                        ("$exists", Value::Bool(b)) => Cond::Exists(expr.clone(), *b),
                        ("$not", arg) => Cond::NotIn(expr.clone(), values(&key, arg)?),
                        (op, arg) => {
                            return Err(crate::l10n!(
                                "match `{key}`: unknown `{op}: {arg:?}` (use $exists: true|false or $not)",
                                "match `{key}`: 모르는 `{op}: {arg:?}`($exists: true|false 또는 $not)"
                            ))
                        }
                    });
                }
            }
            want => out.push(Cond::In(expr, values(&key, want)?)),
        }
    }
    Ok(out)
}

/// 값 하나 또는 목록. YAML 불리언·숫자는 글자로 비교한다. null은 `$exists: false`로 쓴다.
fn values(key: &str, v: &Value) -> Result<Vec<String>, String> {
    let items = match v {
        Value::Sequence(items) => items.as_slice(),
        v => std::slice::from_ref(v),
    };
    items
        .iter()
        .map(|v| match v {
            Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(yaml_scalar(v)),
            Value::Null => Err(crate::l10n!(
                "match `{key}`: write null as {{$exists: false}}",
                "match `{key}`: null은 {{$exists: false}}로 씁니다"
            )),
            _ => Err(crate::l10n!(
                "match `{key}`: expected a value or a list of values, got {v:?}",
                "match `{key}`: 값이나 값 목록이 와야 하는데 {v:?}가 왔습니다"
            )),
        })
        .collect()
}

pub fn all(conds: &[Cond], env: &Env) -> bool {
    // 값은 옛 `dig_string`처럼 글자로: 문자열은 그대로, 그 밖은 JSON
    let text = |e: &Expr| {
        e.eval(env).map(|v| match v {
            serde_json::Value::String(s) => s,
            v => v.to_string(),
        })
    };
    conds.iter().all(|c| match c {
        Cond::In(e, want) => text(e).is_some_and(|t| want.contains(&t)),
        Cond::NotIn(e, want) => !text(e).is_some_and(|t| want.contains(&t)),
        Cond::Exists(e, want) => e.eval(env).is_some() == *want,
        Cond::Any(groups) => groups.iter().any(|g| all(g, env)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn no_ctx(_: &str) -> Option<String> {
        None
    }
    fn no_side(_: &str) -> Option<serde_json::Value> {
        None
    }

    fn env<'a>(outer: &'a serde_json::Value, elem: Option<&'a serde_json::Value>) -> Env<'a> {
        Env {
            outer,
            elem,
            key: None,
            index: None,
            file: None,
            root_dir: None,
            ctx: &no_ctx,
            side: &no_side,
            json: Default::default(),
        }
    }

    fn conds(yaml: &str, legacy: bool) -> Result<Vec<Cond>, String> {
        parse(&serde_yaml::from_str(yaml).unwrap(), legacy)
    }

    /// 조건 YAML이 레코드에 맞는가.
    fn hit(yaml: &str, rec: serde_json::Value) -> bool {
        all(&conds(yaml, false).unwrap(), &env(&rec, None))
    }

    #[test]
    fn equal_and_one_of() {
        assert!(hit("{type: message}", json!({"type": "message"})));
        assert!(!hit("{type: message}", json!({"type": "other"})));
        let role = "{message.role: [assistant, ai]}";
        assert!(hit(role, json!({"message": {"role": "ai"}})));
        assert!(!hit(role, json!({"message": {"role": "user"}})));
    }

    #[test]
    fn exists() {
        let yes = "{message.model: {$exists: true}}";
        let no = "{message.model: {$exists: false}}";
        let with = json!({"message": {"model": "m"}});
        assert!(hit(yes, with.clone()) && !hit(no, with));
        for without in [
            json!({}),
            json!({"message": {"model": null}}),
            json!({"message": {"model": ""}}),
        ] {
            assert!(
                !hit(yes, without.clone()) && hit(no, without),
                "없음·null·빈 글자는 값 없음"
            );
        }
    }

    #[test]
    fn not() {
        let one = "{message.api: {$not: openclaw-transcript}}";
        assert!(!hit(
            one,
            json!({"message": {"api": "openclaw-transcript"}})
        ));
        assert!(hit(one, json!({"message": {"api": "chat"}})));
        let many = "{kind: {$not: [a, b]}}";
        assert!(!hit(many, json!({"kind": "a"})) && !hit(many, json!({"kind": "b"})));
        assert!(hit(many, json!({"kind": "c"})));
    }

    #[test]
    fn missing_value_fails_in_and_passes_not() {
        assert!(!hit("{type: message}", json!({})));
        assert!(!hit("{type: [a, b]}", json!({})));
        assert!(hit("{type: {$not: message}}", json!({})));
    }

    #[test]
    fn any_of_groups() {
        let y = "{$any: [{variant: ai}, {role: assistant, done: true}]}";
        assert!(hit(y, json!({"variant": "ai"})));
        assert!(hit(y, json!({"role": "assistant", "done": true})));
        assert!(!hit(y, json!({"role": "assistant"})), "묶음 안은 AND");
        assert!(!hit(y, json!({"variant": "human"})));
    }

    #[test]
    fn items_are_anded() {
        let y = "{type: event_msg, payload.type: token_count, payload.info: {$exists: true}}";
        let rec = json!({"type": "event_msg", "payload": {"type": "token_count", "info": {}}});
        assert!(hit(y, rec));
        assert!(!hit(
            y,
            json!({"type": "event_msg", "payload": {"type": "token_count"}})
        ));
        assert!(!hit(
            y,
            json!({"type": "x", "payload": {"type": "token_count", "info": {}}})
        ));
    }

    #[test]
    fn keys_are_path_expressions() {
        let otel = r#"{'attributes["gen_ai.operation.name"]': chat}"#;
        assert!(hit(
            otel,
            json!({"attributes": {"gen_ai.operation.name": "chat"}})
        ));
        assert!(hit(
            "{data@json.role: assistant}",
            json!({"data": "{\"role\": \"assistant\"}"})
        ));
        // each 원소 안: 이름은 원소, `$.`은 바깥 레코드
        let c = conds("{$.type: assistant, type: advisor_message}", false).unwrap();
        let outer = json!({"type": "assistant"});
        let elem = json!({"type": "advisor_message"});
        assert!(all(&c, &env(&outer, Some(&elem))));
        assert!(!all(&c, &env(&elem, Some(&elem))));
    }

    #[test]
    fn yaml_scalars_compare_as_text() {
        assert!(hit("{enabled: true}", json!({"enabled": true})));
        assert!(hit("{enabled: true}", json!({"enabled": "true"})));
        assert!(!hit("{enabled: true}", json!({"enabled": false})));
        assert!(hit("{code: 7}", json!({"code": 7})));
        assert!(hit("{code: 7}", json!({"code": "7"})));
        assert!(hit("{code: [7, 8]}", json!({"code": 8})));
    }

    #[test]
    fn null_is_legacy_only() {
        assert!(conds("{X: null}", false).is_err());
        let c = conds("{X: null}", true).unwrap();
        assert!(matches!(c[..], [Cond::Exists(_, false)]));
        let legacy = conds("{a.0.b: x}", true).unwrap();
        assert!(
            all(&legacy, &env(&json!({"a": [{"b": "x"}]}), None)),
            "옛 번호 경로"
        );
    }

    #[test]
    fn bad_conditions_are_errors() {
        for bad in [
            "{a: {$exists: yes}}",
            "{a: {$in: [x]}}",
            "{a: {$not: {b: c}}}",
            "{a: [x, null]}",
            "{a: [[x]]}",
            "{$any: {a: b}}",
            "{$any: [a]}",
            "{'a[': x}",
            "{$nope.x: y}",
        ] {
            assert!(conds(bad, false).is_err(), "{bad}");
        }
    }
}
