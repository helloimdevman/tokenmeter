//! 경로식(F3). 스펙을 읽을 때 한 번 파싱해 트리로 두고, 레코드마다 평가한다.
//! 옛 `dig`·`dig_string`·`num`·`is_truthy`는 W2가 모든 자리를 옮길 때까지 둔다.

use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub struct FileVars {
    pub path: String,
    pub name: String,
    pub stem: String,
    pub dirs: Vec<String>,
}

/// 식을 평가할 때의 자리. `outer`는 `$`(바깥 레코드), `elem`은 `each` 원소(없으면 outer).
pub struct Env<'a> {
    pub outer: &'a Value,
    pub elem: Option<&'a Value>,
    pub key: Option<&'a str>, // $key
    pub index: Option<usize>, // $index
    pub file: Option<&'a FileVars>,
    pub root_dir: Option<&'a str>, // $root
    pub ctx: &'a dyn Fn(&str) -> Option<String>,
    pub side: &'a dyn Fn(&str) -> Option<Value>,
    /// `@json`으로 푼 값. 한 레코드에서 같은 문자열을 두 번 풀지 않는다.
    /// ponytail: 원문 문자열이 키라 찾을 때마다 문자열을 해시한다. 풀기보다 훨씬 싸다.
    /// 포인터 키는 `$ctx`·`$key` 같은 임시 값의 주소가 다시 쓰여 틀린 값을 줄 수 있어 쓰지 않았다.
    pub json: RefCell<HashMap<String, Rc<Value>>>,
}

impl Env<'_> {
    fn parsed(&self, s: &str) -> Option<Rc<Value>> {
        if let Some(v) = self.json.borrow().get(s) {
            return Some(v.clone());
        }
        let v: Rc<Value> = Rc::new(serde_json::from_str(s).ok()?);
        self.json.borrow_mut().insert(s.to_string(), v.clone());
        Some(v)
    }
}

#[derive(Clone, Debug)]
pub struct Expr(Node);

#[derive(Clone, Debug)]
enum Node {
    Path(Base, Vec<Step>),
    Bin(Box<Node>, Op, Box<Node>),
    Const(f64),
}

#[derive(Clone, Copy, Debug)]
enum Op {
    Add,
    Sub,
    Mul,
    Cat,
}

#[derive(Clone, Debug)]
enum Base {
    Cur,
    Outer,
    Key,
    Index,
    Root,
    File(String),
    Ctx(String),
    Side(String),
}

#[derive(Clone, Debug)]
enum Step {
    Key(String),
    Index(i64),
    Dyn(Box<Node>),
    All,
    Filter(Box<Node>),
    Json,
}

impl Expr {
    pub fn parse(src: &str) -> Result<Expr, String> {
        let mut p = Parser {
            s: src.trim(),
            i: 0,
        };
        let node = p.expr()?;
        if p.i < p.s.len() {
            return p.err("unexpected text");
        }
        Ok(Expr(node))
    }

    /// 값이 없음 = None(경로 없음, null, 빈 문자열). `[*]`는 첫 값.
    pub fn eval(&self, env: &Env) -> Option<Value> {
        self.all(env).into_iter().next()
    }

    fn all(&self, env: &Env) -> Vec<Value> {
        eval(&self.0, env.elem.unwrap_or(env.outer), env)
    }
}

/// YAML 식 자리 하나. 목록이면 "앞에서부터 처음 값이 있는 것"(스펙 2.0).
#[derive(Clone, Debug)]
pub struct Pick(Vec<Expr>);

impl Pick {
    pub fn from_yaml(v: &serde_yaml::Value) -> Result<Pick, String> {
        let one = |v: &serde_yaml::Value| {
            v.as_str()
                .ok_or_else(|| format!("expected a path expression, got {v:?}"))
                .and_then(Expr::parse)
        };
        match v {
            serde_yaml::Value::Sequence(items) => {
                items.iter().map(one).collect::<Result<_, _>>().map(Pick)
            }
            v => one(v).map(|e| Pick(vec![e])),
        }
    }

    pub fn value(&self, env: &Env) -> Option<Value> {
        self.0.iter().find_map(|e| e.eval(env))
    }

    /// 숫자 문자열 허용. `[*]`는 합.
    pub fn num(&self, env: &Env) -> Option<f64> {
        self.0.iter().find_map(|e| num_of(&e.all(env)))
    }

    /// 객체·배열은 압축 JSON.
    pub fn text(&self, env: &Env) -> Option<String> {
        self.0.iter().find_map(|e| text_of(&e.all(env)))
    }

    pub fn truthy(&self, env: &Env) -> bool {
        self.value(env).is_some_and(|v| is_truthy(&v))
    }
}

/// 옛 `a.0.b`를 `a[0].b`로. 사용자 덮어쓰기에서만 부른다(스펙 1절).
pub fn legacy_path(s: &str) -> String {
    let mut out = String::new();
    for part in s.split('.') {
        if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
            out += &format!("[{part}]");
        } else {
            if !out.is_empty() {
                out.push('.');
            }
            out += part;
        }
    }
    out
}

struct Parser<'s> {
    s: &'s str,
    i: usize,
}

impl Parser<'_> {
    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!(
            "path expression `{}`: {what} at byte {}",
            self.s, self.i
        ))
    }

    fn peek(&self) -> Option<u8> {
        self.s.as_bytes().get(self.i).copied()
    }

    fn eat(&mut self, t: &str) -> bool {
        let hit = self.s[self.i..].starts_with(t);
        if hit {
            self.i += t.len();
        }
        hit
    }

    fn spaces(&mut self) -> usize {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
        self.i - start
    }

    /// 키 이름. `.[]@`와 공백에서 끊는다(그래서 `cache-read`는 키 하나).
    fn name(&mut self) -> String {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| !matches!(c, b'.' | b'[' | b']' | b'@') && !c.is_ascii_whitespace())
        {
            self.i += 1;
        }
        self.s[start..self.i].to_string()
    }

    /// 항 (공백 연산자 공백 항)*. 연산자는 앞뒤에 공백이 있을 때만이고 왼쪽부터 묶는다.
    fn expr(&mut self) -> Result<Node, String> {
        let mut left = self.term()?;
        loop {
            let save = self.i;
            let op = match (self.spaces() > 0, self.peek()) {
                (true, Some(b'+')) => Op::Add,
                (true, Some(b'-')) => Op::Sub,
                (true, Some(b'*')) => Op::Mul,
                (true, Some(b'&')) => Op::Cat,
                _ => {
                    self.i = save;
                    return Ok(left);
                }
            };
            self.i += 1;
            if self.spaces() == 0 {
                return self.err("operator needs spaces on both sides and a right side");
            }
            let right = match op {
                Op::Mul => self.number()?,
                _ => self.term()?,
            };
            left = Node::Bin(Box::new(left), op, Box::new(right));
        }
    }

    fn term(&mut self) -> Result<Node, String> {
        let mut steps = vec![];
        let base = if self.eat("$") {
            let var = self.name();
            let sub = |p: &mut Self| {
                let n = if p.eat(".") { p.name() } else { String::new() };
                if n.is_empty() {
                    return p.err(&format!("`${var}` needs `.name`"));
                }
                Ok(n)
            };
            match var.as_str() {
                "" => Base::Outer,
                "key" => Base::Key,
                "index" => Base::Index,
                "root" => Base::Root,
                "ctx" => Base::Ctx(sub(self)?),
                "side" => Base::Side(sub(self)?),
                "file" => match sub(self)?.as_str() {
                    f @ ("path" | "name" | "stem" | "dir") => Base::File(f.into()),
                    _ => return self.err("`$file` has path, name, stem, dir"),
                },
                _ => return self.err(&format!("unknown variable `${var}`")),
            }
        } else if self.peek() == Some(b'[') {
            Base::Cur // 맨 앞 대괄호는 뿌리 객체의 키
        } else {
            let n = self.name();
            if n.is_empty() {
                return self.err("expected a path");
            }
            steps.push(Step::Key(n));
            Base::Cur
        };
        loop {
            match self.peek() {
                Some(b'.') => {
                    self.i += 1;
                    let n = self.name();
                    if n.is_empty() {
                        return self.err("expected a key after `.`");
                    }
                    steps.push(Step::Key(n));
                }
                Some(b'[') => {
                    self.i += 1;
                    steps.push(self.bracket()?);
                }
                Some(b'@') => {
                    if !self.eat("@json") {
                        return self.err("only `@json` is known");
                    }
                    steps.push(Step::Json);
                }
                _ => return Ok(Node::Path(base, steps)),
            }
        }
    }

    /// `[` 다음부터 `]`까지: 번호, `"키"`, `$식`, `*`, `?식`.
    fn bracket(&mut self) -> Result<Step, String> {
        let step = match self.peek() {
            Some(b'*') => {
                self.i += 1;
                Step::All
            }
            Some(b'?') => {
                self.i += 1;
                Step::Filter(Box::new(self.expr()?))
            }
            Some(b'$') => Step::Dyn(Box::new(self.expr()?)),
            Some(b'"') => {
                self.i += 1;
                let Some(len) = self.s[self.i..].find('"') else {
                    return self.err("unclosed `\"`");
                };
                let key = self.s[self.i..self.i + len].to_string();
                self.i += len + 1;
                Step::Key(key)
            }
            _ => {
                let start = self.i;
                self.eat("-");
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.i += 1;
                }
                match self.s[start..self.i].parse() {
                    Ok(n) => Step::Index(n),
                    Err(_) => return self.err("expected an index, \"key\", $expr, * or ?expr"),
                }
            }
        };
        if !self.eat("]") {
            return self.err("expected `]`");
        }
        Ok(step)
    }

    /// `*`의 오른쪽 숫자 상수.
    fn number(&mut self) -> Result<Node, String> {
        let start = self.i;
        while self
            .peek()
            .is_some_and(|c| !c.is_ascii_whitespace() && c != b']')
        {
            self.i += 1;
        }
        match self.s[start..self.i].parse::<f64>() {
            Ok(f) if f.is_finite() => Ok(Node::Const(f)),
            _ => {
                self.i = start;
                self.err("`*` takes a number on the right")
            }
        }
    }
}

/// 값 목록을 낸다. 보통은 하나, `[*]`를 지나면 여럿, 값이 없으면 빈 목록.
fn eval(node: &Node, cur: &Value, env: &Env) -> Vec<Value> {
    match node {
        Node::Const(f) => number(*f).into_iter().collect(),
        Node::Path(base, steps) => {
            let owned;
            let start = match base {
                Base::Cur => cur,
                Base::Outer => env.outer,
                var => match variable(var, env) {
                    Some(v) => {
                        owned = v;
                        &owned
                    }
                    None => return vec![],
                },
            };
            let mut out = vec![];
            walk(start, steps, env, &mut out);
            out
        }
        Node::Bin(l, op, r) => {
            let (l, r) = (eval(l, cur, env), eval(r, cur, env));
            let v = match op {
                Op::Add => match (num_of(&l), num_of(&r)) {
                    (None, None) => None,
                    (a, b) => number(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
                },
                Op::Sub => num_of(&l).zip(num_of(&r)).and_then(|(a, b)| number(a - b)),
                Op::Mul => num_of(&l).zip(num_of(&r)).and_then(|(a, b)| number(a * b)),
                Op::Cat => text_of(&l)
                    .zip(text_of(&r))
                    .map(|(a, b)| Value::String(format!("{a}|{b}"))),
            };
            v.into_iter().collect()
        }
    }
}

fn variable(base: &Base, env: &Env) -> Option<Value> {
    Some(match base {
        Base::Key => env.key?.into(),
        Base::Index => env.index?.into(),
        Base::Root => env.root_dir?.into(),
        Base::Ctx(n) => (env.ctx)(n)?.into(),
        Base::Side(n) => (env.side)(n)?,
        Base::File(part) => {
            let f = env.file?;
            match part.as_str() {
                "path" => f.path.as_str().into(),
                "name" => f.name.as_str().into(),
                "stem" => f.stem.as_str().into(),
                _ => f.dirs.clone().into(),
            }
        }
        Base::Cur | Base::Outer => return None,
    })
}

fn walk(cur: &Value, steps: &[Step], env: &Env, out: &mut Vec<Value>) {
    let Some((step, rest)) = steps.split_first() else {
        if !cur.is_null() && cur.as_str() != Some("") {
            out.push(cur.clone());
        }
        return;
    };
    match step {
        Step::Key(k) => {
            if let Some(v) = cur.get(k) {
                walk(v, rest, env, out)
            }
        }
        Step::Index(n) => {
            if let Some(v) = cur.as_array().and_then(|a| nth(a, *n)) {
                walk(v, rest, env, out)
            }
        }
        Step::Dyn(e) => {
            let key = text_of(&eval(e, env.elem.unwrap_or(env.outer), env));
            let v = key.and_then(|k| match cur {
                Value::Array(a) => nth(a, k.parse().ok()?),
                _ => cur.get(&k),
            });
            if let Some(v) = v {
                walk(v, rest, env, out)
            }
        }
        Step::All => {
            for v in elems(cur) {
                walk(v, rest, env, out)
            }
        }
        Step::Filter(c) => {
            let hits: Vec<&Value> = elems(cur)
                .into_iter()
                .filter(|v| eval(c, v, env).first().is_some_and(is_truthy))
                .collect();
            // 뒤에 번호가 오면 그 번호, 아니면 첫 원소
            let (n, rest) = match rest.split_first() {
                Some((Step::Index(n), r)) => (*n, r),
                _ => (0, rest),
            };
            if let Some(v) = nth(&hits, n) {
                walk(v, rest, env, out)
            }
        }
        Step::Json => {
            if let Some(v) = cur.as_str().and_then(|s| env.parsed(s)) {
                walk(&v, rest, env, out)
            }
        }
    }
}

/// 배열은 원소, 맵은 값.
fn elems(v: &Value) -> Vec<&Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(m) => m.values().collect(),
        _ => vec![],
    }
}

/// 음수는 뒤에서 센다.
fn nth<T>(items: &[T], n: i64) -> Option<&T> {
    let i = if n < 0 {
        items.len().checked_sub(n.unsigned_abs() as usize)?
    } else {
        n as usize
    };
    items.get(i)
}

fn number(f: f64) -> Option<Value> {
    serde_json::Number::from_f64(f).map(Value::Number)
}

/// 숫자 자리: 숫자 문자열 허용, 여럿이면 합.
fn num_of(vals: &[Value]) -> Option<f64> {
    vals.iter()
        .filter_map(|v| match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        })
        .filter(|f| f.is_finite())
        .reduce(|a, b| a + b)
}

/// 그 밖의 자리: 첫 값. 객체·배열은 압축 JSON(`dig_string`과 같음).
fn text_of(vals: &[Value]) -> Option<String> {
    Some(match vals.first()? {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    })
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec() -> Value {
        json!({"message": {"id": "m1", "usage": {"input_tokens": 3, "output_tokens": "5"}},
               "requestId": "r1", "messages": [{"id": "a"}, {"id": "b"}],
               "attributes": {"gen_ai.usage.input_tokens": 7},
               "ws": {"workspaces": {"w1": {"root": "/r"}}},
               "modelUsage": {"m-a": {"outputTokens": 2}, "m-b": {"outputTokens": 4}},
               "nodes": [{"u": {"o": 0}}, {"u": {"o": 9}}, {"u": {"o": 0}}],
               "text": "{\"tokensIn\": 11}", "time": {"created": 100, "completed": 250},
               "usage": {"input": 1, "orchestration": {"input": 2}, "costUsdTicks": 30000000000i64},
               "empty": "", "nothing": null})
    }

    fn ctx(name: &str) -> Option<String> {
        match name {
            "session" => Some("s1".into()),
            "vendor" => Some("anthropic".into()),
            _ => None,
        }
    }

    fn side(name: &str) -> Option<Value> {
        match name {
            "root" => Some(json!("/r")),
            "ws" => Some(json!({"workspaces": {"sess-9": {"root": "/w"}}})),
            _ => None,
        }
    }

    fn env(r: &Value) -> Env<'_> {
        let file = Box::leak(Box::new(FileVars {
            path: "/h/a/sess-9/log.jsonl".into(),
            name: "log.jsonl".into(),
            stem: "log".into(),
            dirs: vec!["a".into(), "sess-9".into()],
        }));
        Env {
            outer: r,
            elem: None,
            key: None,
            index: None,
            file: Some(file),
            root_dir: Some("/h"),
            ctx: &ctx,
            side: &side,
            json: Default::default(),
        }
    }

    fn one(s: &str) -> Pick {
        Pick::from_yaml(&serde_yaml::Value::String(s.into())).unwrap()
    }
    fn ev(e: &Env, s: &str) -> Option<Value> {
        Expr::parse(s).unwrap().eval(e)
    }
    fn num(e: &Env, s: &str) -> Option<f64> {
        one(s).num(e)
    }

    #[test]
    fn grammar_table() {
        let r = rec();
        let e = env(&r);
        let v = |s: &str| Expr::parse(s).unwrap().eval(&e);
        assert_eq!(v("message.usage.input_tokens"), Some(json!(3)));
        assert_eq!(v("messages[0].id"), Some(json!("a")));
        assert_eq!(v("messages[-1].id"), Some(json!("b")));
        assert_eq!(
            v(r#"attributes["gen_ai.usage.input_tokens"]"#),
            Some(json!(7))
        );
        assert_eq!(
            v("ws.workspaces[$file.dir[-1]].root"),
            None,
            "sess-9 키 없음"
        );
        assert_eq!(
            num(&e, "modelUsage[*].outputTokens"),
            Some(6.0),
            "숫자 자리는 합"
        );
        assert_eq!(num(&e, "nodes[?u.o][-1].u.o"), Some(9.0));
        assert_eq!(v("text@json.tokensIn"), Some(json!(11)));
        assert_eq!(v("$.requestId"), Some(json!("r1")));
        assert_eq!(v("$file.dir[-1]"), Some(json!("sess-9")));
        assert_eq!(v("$ctx.session & message.id"), Some(json!("s1|m1")));
        assert_eq!(
            num(&e, "usage.input + usage.orchestration.input"),
            Some(3.0)
        );
        assert_eq!(
            num(&e, "usage.input + usage.missing"),
            Some(1.0),
            "+는 한쪽이 없으면 0"
        );
        assert_eq!(num(&e, "time.completed - time.created"), Some(150.0));
        assert_eq!(
            num(&e, "time.completed - time.missing"),
            None,
            "-는 한쪽이 없으면 값 없음"
        );
        assert_eq!(num(&e, "usage.costUsdTicks * 0.0000000001"), Some(3.0));
        assert_eq!(v("message.id & missing"), None, "&는 모든 부분이 있어야");
        assert_eq!(v("empty"), None);
        assert_eq!(v("nothing"), None);
        assert_eq!(
            num(&e, "message.usage.output_tokens"),
            Some(5.0),
            "숫자 문자열"
        );
    }

    #[test]
    fn lists_filters_and_operators() {
        let r = rec();
        let e = env(&r);
        assert_eq!(
            ev(&e, "messages[*].id"),
            Some(json!("a")),
            "숫자 밖 자리는 첫 값"
        );
        assert_eq!(
            ev(&e, "nodes[?u.o].u.o"),
            Some(json!(9)),
            "번호 없으면 첫 원소"
        );
        assert_eq!(
            num(&e, "usage.missing + usage.nope"),
            None,
            "+는 둘 다 없으면 값 없음"
        );
        assert_eq!(
            num(&e, "time.created - time.completed"),
            Some(-150.0),
            "식 중간 음수는 그대로"
        );
        assert_eq!(num(&e, "usage.missing * 2"), None);
        assert_eq!(
            num(&e, "usage.input + usage.input * 10"),
            Some(20.0),
            "왼쪽부터 묶는다"
        );
        assert_eq!(
            ev(&e, "$ctx.session & requestId & message.id"),
            Some(json!("s1|r1|m1"))
        );
        assert_eq!(
            one("ws.workspaces").text(&e).as_deref(),
            Some(r#"{"w1":{"root":"/r"}}"#)
        );
        assert_eq!(
            one("message.usage.input_tokens").text(&e).as_deref(),
            Some("3")
        );
    }

    #[test]
    fn variables() {
        let r = rec();
        let mut e = env(&r);
        assert_eq!(ev(&e, "$key"), None, "each 밖");
        assert_eq!(ev(&e, "$index"), None, "each 밖");
        assert_eq!(ev(&e, "$root"), Some(json!("/h")));
        assert_eq!(ev(&e, "$file.name"), Some(json!("log.jsonl")));
        assert_eq!(ev(&e, "$file.stem"), Some(json!("log")));
        assert_eq!(ev(&e, "$file.path"), Some(json!("/h/a/sess-9/log.jsonl")));
        assert_eq!(ev(&e, "$side.root"), Some(json!("/r")));
        assert_eq!(
            ev(&e, "$side.ws.workspaces[$file.dir[-1]].root"),
            Some(json!("/w"))
        );
        assert_eq!(ev(&e, "$ctx.nope"), None);
        let el = json!({"inputTokens": 4});
        e.elem = Some(&el);
        e.key = Some("m-a");
        e.index = Some(1);
        assert_eq!(ev(&e, "$key"), Some(json!("m-a")));
        assert_eq!(ev(&e, "$index"), Some(json!(1)));
        assert_eq!(ev(&e, "inputTokens"), Some(json!(4)), "이름은 원소 기준");
        assert_eq!(ev(&e, "$.requestId"), Some(json!("r1")), "$는 바깥 레코드");
    }

    #[test]
    fn leading_bracket_is_a_root_key() {
        let r = json!({"x": {"y": 1}, "anthropic": {"type": "oauth"}});
        let e = env(&r);
        assert_eq!(ev(&e, r#"["x"].y"#), Some(json!(1)));
        assert_eq!(ev(&e, "[$ctx.vendor].type"), Some(json!("oauth")));
    }

    #[test]
    fn dash_without_spaces_is_part_of_a_key() {
        let r = json!({"cache-read": 5, "a": 2, "b-c": 1});
        let e = env(&r);
        assert_eq!(num(&e, "cache-read"), Some(5.0));
        assert_eq!(num(&e, "a - b-c"), Some(1.0));
    }

    #[test]
    fn pick_list_takes_the_first_value() {
        let r = rec();
        let e = env(&r);
        let p = |y: &str| Pick::from_yaml(&serde_yaml::from_str(y).unwrap()).unwrap();
        assert_eq!(
            p("[missing, empty, nothing, message.id, requestId]").value(&e),
            Some(json!("m1"))
        );
        assert_eq!(p("[missing, requestId]").text(&e).as_deref(), Some("r1"));
        assert_eq!(
            p("[usage.missing, 'time.completed - time.created']").num(&e),
            Some(150.0)
        );
        assert!(!p("[nothing, empty]").truthy(&e));
        assert!(p("[nothing, message.id]").truthy(&e));
        assert!(!p("nodes[0].u.o").truthy(&e), "0은 거짓");
        assert!(Pick::from_yaml(&serde_yaml::from_str("5").unwrap()).is_err());
    }

    #[test]
    fn json_is_parsed_once_per_record() {
        let r = json!({"data": "{\"a\": 1, \"b\": \"{\\\"c\\\": 2}\"}"});
        let e = env(&r);
        assert_eq!(ev(&e, "data@json.a"), Some(json!(1)));
        assert_eq!(ev(&e, "data@json.b@json.c"), Some(json!(2)));
        assert_eq!(e.json.borrow().len(), 2, "data 한 번, b 한 번");
        assert_eq!(ev(&e, "data@json.a@json"), None, "문자열이 아니면 값 없음");
    }

    #[test]
    fn parse_errors() {
        for bad in [
            "a[",
            "a +",
            "$nope.x",
            "a[?]",
            "",
            "a b",
            "a * b",
            "a@yaml",
            "a.",
            "$file.size",
            "a[x]",
            r#"a["x"#,
        ] {
            assert!(Expr::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn legacy_dot_indexes() {
        assert_eq!(legacy_path("a.0.b"), "a[0].b");
        assert_eq!(legacy_path("0.a"), "[0].a");
        assert_eq!(
            legacy_path("message.usage.input_tokens"),
            "message.usage.input_tokens"
        );
    }
}
