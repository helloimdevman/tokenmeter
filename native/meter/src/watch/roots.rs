//! 경로 틀과 루트(F10, B4), 레코드 밖 파일 읽기(스펙 2.0).
//!
//! 경로 틀의 `$`는 한 가지 뜻이다. `$TOKENMETER_HOME`(데이터 디렉터리), `$root`, `$ctx.<이름>`은
//! 예약 이름이고 나머지 `$VAR`·`${VAR:-기본}`은 환경 변수다. 맨 앞 `~`는 HOME이다.

use super::expr::{Env, Pick};
use glob::{MatchOptions, Pattern};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use tokenmeter_hook::data_dir;

/// 레코드 밖 파일 상한(스펙 2.0).
const OUTSIDE_MAX: u64 = 1 << 20;

pub struct Vars<'a> {
    pub root: Option<&'a Path>,
    pub ctx: &'a dyn Fn(&str) -> Option<String>,
}

/// `~`, `$TOKENMETER_HOME`, `$VAR`, `${VAR:-기본}`, `$root`, `$ctx.x`를 편다.
/// Ok(None) = 기본값 없는 변수가 없거나 비어 버린다(B4). 환경 변수 값에 쉼표가 있으면 여러 경로.
pub fn expand(template: &str, vars: &Vars) -> Result<Option<Vec<PathBuf>>, String> {
    Ok(expand_str(template, vars)?.map(|all| all.into_iter().map(PathBuf::from).collect()))
}

fn expand_str(s: &str, vars: &Vars) -> Result<Option<Vec<String>>, String> {
    let mut outs = vec![String::new()];
    let mut rest = s;
    if rest == "~" || rest.starts_with("~/") {
        let Some(home) = home_dir() else {
            return Ok(None);
        };
        outs[0] = home.to_string_lossy().into_owned();
        rest = &rest[1..];
    }
    while let Some(at) = rest.find('$') {
        let after = &rest[at + 1..];
        let (name, default, next) = if let Some(inner) = after.strip_prefix('{') {
            let close = closing_brace(inner).ok_or_else(|| {
                crate::l10n!("unclosed ${{ in {s}", "{s}의 ${{가 닫히지 않았다", s = s)
            })?;
            let body = &inner[..close];
            let (name, default) = match body.split_once(":-") {
                Some((name, default)) => (name, Some(default)),
                None => (body, None),
            };
            (name, default, &inner[close + 1..])
        } else {
            let n = bare_name(after);
            (&after[..n], None, &after[n..])
        };
        check_name(name, s)?;
        let mut values = match (lookup(name, vars), default) {
            (Some(values), _) => values,
            (None, Some(default)) => match expand_str(default, vars)? {
                Some(values) => values,
                None => return Ok(None),
            },
            (None, None) => return Ok(None),
        };
        values.retain(|v| !v.is_empty());
        if values.is_empty() {
            return Ok(None);
        }
        let head = &rest[..at];
        outs = outs
            .iter()
            .flat_map(|o| values.iter().map(move |v| format!("{o}{head}{v}")))
            .collect();
        rest = next;
    }
    for o in &mut outs {
        o.push_str(rest);
    }
    Ok(Some(outs))
}

/// 짝이 맞는 `}`의 위치(`${A:-${B}}`처럼 겹칠 수 있다).
fn closing_brace(s: &str) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in s.char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 0 => return Some(i),
            '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// `$` 뒤 이름의 길이. `$ctx.<이름>`은 점까지 한 이름이다.
fn bare_name(s: &str) -> usize {
    let ident = |s: &str| {
        s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(s.len())
    };
    let n = ident(s);
    if &s[..n] == "ctx" && s[n..].starts_with('.') {
        n + 1 + ident(&s[n + 1..])
    } else {
        n
    }
}

fn check_name(name: &str, template: &str) -> Result<(), String> {
    let id = name.strip_prefix("ctx.").unwrap_or(name);
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Ok(());
    }
    Err(crate::l10n!(
        "bad variable \"{name}\" in {template}",
        "{template}의 변수 \"{name}\"를 읽을 수 없다",
        name = name,
        template = template
    ))
}

/// 변수 값. 없거나 비면 None. 환경 변수만 쉼표 목록을 나눈다.
fn lookup(name: &str, vars: &Vars) -> Option<Vec<String>> {
    let values = match name {
        "TOKENMETER_HOME" => vec![data_dir().to_string_lossy().into_owned()],
        "root" => vec![vars.root?.to_string_lossy().into_owned()],
        _ => match name.strip_prefix("ctx.") {
            Some(key) => vec![(vars.ctx)(key)?],
            None => std::env::var(name)
                .ok()?
                .split(',')
                .map(|v| v.trim().to_string())
                .collect(),
        },
    };
    let values: Vec<String> = values.into_iter().filter(|v| !v.is_empty()).collect();
    (!values.is_empty()).then_some(values)
}

/// 옛 이름(`quota.rs`·설치·프로브가 쓴다). 첫 경로만 준다. 펼 수 없으면 빈 경로라 읽기·쓰기가 실패한다.
pub(crate) fn expand_home(raw: &str) -> PathBuf {
    let vars = Vars {
        root: None,
        ctx: &|_| None,
    };
    match expand(raw, &vars) {
        Ok(Some(paths)) => paths.into_iter().next().unwrap_or_default(),
        _ => PathBuf::new(),
    }
}

/// 루트는 글자 그대로의 디렉터리다. `*?[`, 또는 `${`가 아닌 `{`면 오류.
pub fn check_root(s: &str) -> Result<(), String> {
    if s.contains(['*', '?', '[']) {
        return Err(crate::l10n!(
            "root {s} has a wildcard; list each directory as its own root",
            "루트 {s}에 와일드카드가 있다. 디렉터리마다 루트를 따로 적는다",
            s = s
        ));
    }
    check_pattern(s)
}

/// glob 0.3은 중괄호를 모른다. `${`가 아닌 `{`면 오류.
pub fn check_pattern(s: &str) -> Result<(), String> {
    if s.char_indices()
        .any(|(i, c)| c == '{' && !s[..i].ends_with('$'))
    {
        return Err(crate::l10n!(
            "{s} uses {{a,b}}; write one entry per alternative",
            "{s}에 {{a,b}}가 있다. 경우마다 따로 적는다",
            s = s
        ));
    }
    Ok(())
}

/// 루트를 이스케이프하고 패턴을 이어 찾은 파일들.
pub fn glob_under(root: &Path, pattern: &str) -> Vec<PathBuf> {
    let pat = Path::new(&Pattern::escape(&root.to_string_lossy())).join(pattern);
    glob::glob(&pat.to_string_lossy())
        .map(|found| found.flatten().filter(|p| p.is_file()).collect())
        .unwrap_or_default()
}

/// 정규화한 경로로 합친다. 처음 것을 남긴다(없는 경로는 글자 그대로 비교).
pub fn dedup(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|r| seen.insert(fs::canonicalize(r).unwrap_or_else(|_| r.clone())))
        .collect()
}

/// `exclude`는 루트 기준 glob이다. `*`는 `/`를 넘지 않는다.
pub fn excluded(root: &Path, file: &Path, exclude: &[Pattern]) -> bool {
    let Ok(rel) = file.strip_prefix(root) else {
        return false;
    };
    let opts = MatchOptions {
        require_literal_separator: true,
        ..MatchOptions::new()
    };
    exclude.iter().any(|p| p.matches_path_with(rel, opts))
}

/// 다른 앱의 레지스트리에서 읽는 루트(F10). `each`·`path`·`base`는 경로식(F3, F4와 같은 뜻)이다.
pub struct RootsFrom {
    pub file: String,
    pub each: Option<Pick>,
    pub path: Pick,
    pub base: Option<Pick>,
    pub patterns: Vec<String>,
}

impl RootsFrom {
    /// 로딩 때 검증. 루트가 사용자 프로젝트 폴더라 훑을 때마다 트리를 다 걷는 `**`를 막는다.
    pub fn check(&self) -> Result<(), String> {
        for p in &self.patterns {
            check_pattern(p)?;
            if p.contains("**") {
                return Err(crate::l10n!(
                    "roots_from pattern {p} cannot use **",
                    "roots_from 패턴 {p}에는 **를 쓸 수 없다",
                    p = p
                ));
            }
        }
        Ok(())
    }
}

/// 레지스트리에서 프로젝트 루트를 뽑는다. `/`, HOME, HOME의 조상은 버리고 버린 수를 센다.
pub fn roots_from(spec: &RootsFrom, vars: &Vars) -> (Vec<(PathBuf, Vec<String>)>, usize) {
    let home = home_dir().map(|h| fs::canonicalize(&h).unwrap_or(h));
    let text = |doc: &Value, elem: &Value, pick: &Pick| {
        let env = Env {
            elem: Some(elem),
            ..Env::new(doc, vars.ctx)
        };
        pick.text(&env).map(PathBuf::from)
    };
    let (mut out, mut dropped) = (Vec::new(), 0);
    for file in expand(&spec.file, vars).ok().flatten().unwrap_or_default() {
        let Some(doc) = read_outside(&file) else {
            continue;
        };
        let elems: Vec<Value> = match &spec.each {
            None => vec![doc.clone()],
            Some(each) => match each.value(&Env::new(&doc, vars.ctx)) {
                Some(Value::Array(a)) => a,
                Some(Value::Object(m)) => m.into_iter().map(|(_, v)| v).collect(),
                _ => Vec::new(),
            },
        };
        let dir = file.parent().unwrap_or(Path::new(""));
        for elem in &elems {
            let Some(path) = text(&doc, elem, &spec.path) else {
                continue;
            };
            let base = spec.base.as_ref().and_then(|b| text(&doc, elem, b));
            let root = base.as_deref().unwrap_or(dir).join(path);
            let canon = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
            if canon.parent().is_none() || home.as_ref().is_some_and(|h| h.starts_with(&canon)) {
                dropped += 1;
                continue;
            }
            out.push((root, spec.patterns.clone()));
        }
    }
    (out, dropped)
}

/// 스펙 2.0 "레코드 밖 파일 읽기": .toml은 작은 TOML, 그 밖은 JSON(JSONC 주석·끝 쉼표 허용),
/// 둘 다 아니면 앞뒤 공백 뺀 글자. 1 MB 넘으면 None.
pub fn read_outside(path: &Path) -> Option<Value> {
    if fs::metadata(path).ok()?.len() > OUTSIDE_MAX {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    if path.extension().is_some_and(|e| e == "toml") {
        return Some(parse_toml(&text));
    }
    serde_json::from_str(&text)
        .or_else(|_| serde_json::from_str(&strip_jsonc(&text)))
        .ok()
        .or_else(|| Some(Value::String(text.trim().to_string())))
}

/// 문자열 밖의 `//`·`/* */` 주석과 `}`·`]` 앞 끝 쉼표를 뺀다.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_str = false;
    while let Some(c) = chars.next() {
        if in_str {
            out.push(c);
            if c == '\\' {
                out.extend(chars.next());
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                while chars.next().is_some_and(|n| n != '\n') {}
                out.push('\n');
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            ']' | '}' => {
                let end = out.trim_end().len();
                if out[..end].ends_with(',') {
                    out.truncate(end - 1);
                }
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// 작은 TOML: `[a."b.c"]` 섹션과 점 키를 중첩 객체로. 값은 따옴표 문자열·불리언·정수·실수만.
/// ponytail: 배열·인라인 표·`[[배열 표]]`·이스케이프는 건너뛴다. 프로브가 그런 값을 읽게 되면 넓힌다.
fn parse_toml(text: &str) -> Value {
    let mut doc = Value::Object(Map::new());
    let mut section = Some(Vec::new());
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        if line.starts_with("[[") {
            section = None;
            continue;
        }
        if let Some(head) = line.strip_prefix('[') {
            section = head.rfind(']').map(|end| toml_keys(&head[..end]));
            continue;
        }
        let (Some(section), Some((key, value))) = (&section, line.split_once('=')) else {
            continue;
        };
        let Some(value) = toml_value(value.trim()) else {
            continue;
        };
        let path: Vec<String> = section.iter().cloned().chain(toml_keys(key)).collect();
        insert(&mut doc, &path, value);
    }
    doc
}

/// `a."b.c" . d` → [a, b.c, d]. 따옴표 밖 공백은 뺀다.
fn toml_keys(s: &str) -> Vec<String> {
    let mut keys = vec![String::new()];
    let mut quote = None;
    for c in s.chars() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '.') => keys.push(String::new()),
            (None, c) if c.is_whitespace() => {}
            (_, c) => keys.last_mut().expect("keys는 비지 않는다").push(c),
        }
    }
    keys
}

fn toml_value(v: &str) -> Option<Value> {
    if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'') {
        let body = &v[1..];
        return body
            .find(q)
            .map(|end| Value::String(body[..end].to_string()));
    }
    let v = v.split('#').next()?.trim();
    if let Ok(b) = v.parse::<bool>() {
        return Some(Value::Bool(b));
    }
    if let Ok(n) = v.parse::<i64>() {
        return Some(n.into());
    }
    v.parse::<f64>()
        .ok()
        .and_then(serde_json::Number::from_f64)
        .map(Value::Number)
}

fn insert(doc: &mut Value, path: &[String], value: Value) {
    let Some((last, dirs)) = path.split_last() else {
        return;
    };
    let mut cur = doc;
    for key in dirs {
        cur = match cur {
            Value::Object(m) => m
                .entry(key.clone())
                .or_insert_with(|| Value::Object(Map::new())),
            _ => return,
        };
    }
    if let Value::Object(m) = cur {
        m.insert(last.clone(), value);
    }
}

pub(super) fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn no_ctx(_: &str) -> Option<String> {
        None
    }

    fn plain() -> Vars<'static> {
        Vars {
            root: None,
            ctx: &no_ctx,
        }
    }

    fn pick(s: &str) -> Pick {
        Pick::from_yaml(&s.into()).unwrap()
    }

    fn paths(template: &str) -> Option<Vec<PathBuf>> {
        expand(template, &plain()).unwrap()
    }

    /// 환경 변수와 HOME을 건드리지 않는 파일 테스트용 빈 디렉터리.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tm-roots-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn unset_var_without_default_drops_root() {
        std::env::remove_var("TM_ROOTS_NOPE");
        assert_eq!(
            paths("$TM_ROOTS_NOPE/sessions"),
            None,
            "절대 경로 /sessions가 생기지 않는다"
        );
        assert_eq!(paths("${TM_ROOTS_NOPE}/sessions"), None);
        std::env::set_var("TM_ROOTS_EMPTY", "");
        assert_eq!(paths("$TM_ROOTS_EMPTY/sessions"), None, "빈 값도 없는 것");
        assert_eq!(
            paths("${TM_ROOTS_NOPE:-}/sessions"),
            None,
            "빈 기본값도 없는 것"
        );
        assert_eq!(expand_home("$TM_ROOTS_NOPE/sessions"), PathBuf::new());
    }

    #[test]
    fn default_used_when_unset_or_empty() {
        let (_g, root) = crate::test_home("roots-default");
        let home = root.join("home");
        std::env::remove_var("TM_ROOTS_DEF");
        assert_eq!(
            paths("${TM_ROOTS_DEF:-~/.a}/s"),
            Some(vec![home.join(".a/s")])
        );
        std::env::set_var("TM_ROOTS_DEF", "");
        assert_eq!(
            paths("${TM_ROOTS_DEF:-~/.a}/s"),
            Some(vec![home.join(".a/s")])
        );
        std::env::set_var("TM_ROOTS_DEF", "/x");
        assert_eq!(
            paths("${TM_ROOTS_DEF:-~/.a}/s"),
            Some(vec![PathBuf::from("/x/s")])
        );
        std::env::remove_var("TM_ROOTS_DEF");
        std::env::remove_var("TM_ROOTS_DEF2");
        // 기본값 안의 틀도 편다(crush 레지스트리 모양)
        assert_eq!(
            paths("${TM_ROOTS_DEF:-${TM_ROOTS_DEF2:-~/.local/share}/crush}/projects.json"),
            Some(vec![home.join(".local/share/crush/projects.json")])
        );
    }

    #[test]
    fn comma_list_splits() {
        std::env::set_var("TM_ROOTS_LIST", "/a, /b,");
        assert_eq!(
            paths("${TM_ROOTS_LIST:-~/.claude}/projects"),
            Some(vec![
                PathBuf::from("/a/projects"),
                PathBuf::from("/b/projects")
            ])
        );
        assert_eq!(
            expand_home("$TM_ROOTS_LIST"),
            PathBuf::from("/a"),
            "옛 함수는 첫 경로"
        );
    }

    #[test]
    fn tokenmeter_home_and_tilde() {
        let (_g, root) = crate::test_home("roots-home");
        assert_eq!(
            paths("$TOKENMETER_HOME/cursor"),
            Some(vec![root.join("state/cursor")])
        );
        assert_eq!(paths("~/.claude"), Some(vec![root.join("home/.claude")]));
        assert_eq!(paths("~"), Some(vec![root.join("home")]));
        assert_eq!(
            paths("/a/~/b"),
            Some(vec![PathBuf::from("/a/~/b")]),
            "~는 맨 앞만"
        );
        // 예약 이름 $root, $ctx.<이름>
        let ctx = |k: &str| (k == "vendor").then(|| "openai".to_string());
        let vars = Vars {
            root: Some(Path::new("/r")),
            ctx: &ctx,
        };
        assert_eq!(
            expand("$root/../ws/$ctx.vendor.json", &vars).unwrap(),
            Some(vec![PathBuf::from("/r/../ws/openai.json")])
        );
        assert_eq!(
            expand("${ctx.vendor}", &vars).unwrap(),
            Some(vec![PathBuf::from("openai")])
        );
        assert_eq!(
            expand("$ctx.model/x", &vars).unwrap(),
            None,
            "없는 문맥은 버림"
        );
        assert_eq!(paths("$root/x"), None, "$root가 없는 자리");
        // 틀 문법 오류
        for bad in ["${TM_X", "$", "a/$/b", "${TM_X:+y}", "$ctx.", "${}"] {
            assert!(expand(bad, &plain()).is_err(), "{bad}");
        }
    }

    #[test]
    fn braces_or_wildcards_in_root_are_errors() {
        assert!(check_root("~/.{claude,codex}").is_err());
        assert!(check_root("~/a/*/b").is_err());
        assert!(check_root("~/a?/b").is_err());
        assert!(check_root("~/a/[x]").is_err());
        assert!(check_root("${TM_X}/a").is_ok());
        assert!(check_root("${TM_X:-${TM_Y:-~/.a}}/b").is_ok());
        assert!(check_pattern("*.{json,jsonl}").is_err());
        assert!(check_pattern("*/subagents/**/*.jsonl").is_ok());
    }

    #[test]
    fn root_with_brackets_is_escaped() {
        let dir = scratch("brackets");
        let root = dir.join("C#[x]?");
        fs::create_dir_all(root.join("s")).unwrap();
        fs::write(root.join("s/a.jsonl"), "{}").unwrap();
        fs::create_dir_all(root.join("s/d.jsonl")).unwrap();
        assert_eq!(
            glob_under(&root, "*/*.jsonl"),
            vec![root.join("s/a.jsonl")],
            "디렉터리는 빼고"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exclude_is_relative_to_root() {
        let root = Path::new("/r");
        let ex = [glob::Pattern::new("**/subagents/workflows/*/journal.jsonl").unwrap()];
        assert!(excluded(
            root,
            Path::new("/r/p/subagents/workflows/w1/journal.jsonl"),
            &ex
        ));
        assert!(excluded(
            root,
            Path::new("/r/subagents/workflows/w1/journal.jsonl"),
            &ex
        ));
        assert!(!excluded(
            root,
            Path::new("/r/p/subagents/w1/journal.jsonl"),
            &ex
        ));
        assert!(!excluded(root, Path::new("/r/p/s.jsonl"), &ex));
        let top = [glob::Pattern::new("*.jsonl").unwrap()];
        assert!(excluded(root, Path::new("/r/a.jsonl"), &top));
        assert!(
            !excluded(root, Path::new("/r/p/a.jsonl"), &top),
            "*는 /를 넘지 않는다"
        );
        assert!(
            !excluded(root, Path::new("/elsewhere/a.jsonl"), &top),
            "루트 밖"
        );
    }

    #[test]
    fn dedup_by_canonical_path() {
        let dir = scratch("dedup");
        let a = dir.join("a");
        let b = dir.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        let got = dedup(vec![
            a.clone(),
            dir.join("b/../a"),
            b.clone(),
            dir.join("./a/"),
            dir.join("missing"),
            dir.join("missing"),
        ]);
        assert_eq!(got, vec![a, b, dir.join("missing")]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn roots_from_reads_registry_drops_home_ancestors() {
        let (_g, root) = crate::test_home("roots-from");
        let home = root.join("home");
        let reg = home.join("reg");
        fs::create_dir_all(&reg).unwrap();
        let app = home.join("code/C#/app");
        let parent = home.parent().unwrap();
        fs::write(
            reg.join("projects.json"),
            json!({"projects": [
                {"path": app, "data_dir": ".crush"},
                {"path": "/elsewhere", "data_dir": "/abs/.crush"},
                {"path": home, "data_dir": "."},
                {"path": "/", "data_dir": "."},
                {"path": "/x", "data_dir": parent},
                {"path": "/y"},
            ]})
            .to_string(),
        )
        .unwrap();
        let spec = RootsFrom {
            file: "~/reg/projects.json".into(),
            each: Some(pick("projects")),
            path: pick("data_dir"),
            base: Some(pick("path")),
            patterns: vec!["crush.db".into()],
        };
        let (roots, dropped) = roots_from(&spec, &plain());
        let pats = vec!["crush.db".to_string()];
        assert_eq!(
            roots,
            vec![
                (app.join(".crush"), pats.clone()),
                (PathBuf::from("/abs/.crush"), pats)
            ]
        );
        assert_eq!(dropped, 3, "HOME, /, HOME의 조상");

        // base가 없으면 레지스트리 파일의 디렉터리 기준, 맵이면 값마다
        fs::write(
            reg.join("ws.jsonc"),
            r#"{"workspaces": {"w1": {"rootPath": "w1", /* c */}, "w2": {}}}"#,
        )
        .unwrap();
        let spec = RootsFrom {
            file: "~/reg/ws.jsonc".into(),
            each: Some(pick("workspaces")),
            path: pick("rootPath"),
            base: None,
            patterns: vec!["sessions/*/.pi-sessions/*.jsonl".into()],
        };
        let (roots, dropped) = roots_from(&spec, &plain());
        assert_eq!(
            roots.iter().map(|r| r.0.clone()).collect::<Vec<_>>(),
            vec![reg.join("w1")]
        );
        assert_eq!(dropped, 0);

        // 파일이 없거나 틀이 버려지면 빈 목록
        let spec = RootsFrom {
            file: "$TM_ROOTS_NOPE/p.json".into(),
            ..spec
        };
        assert_eq!(roots_from(&spec, &plain()), (Vec::new(), 0));
    }

    #[test]
    fn roots_from_patterns_reject_double_star() {
        let spec = |p: &str| RootsFrom {
            file: "~/r.json".into(),
            each: None,
            path: pick("p"),
            base: None,
            patterns: vec!["crush.db".into(), p.into()],
        };
        assert!(spec("**/crush.db").check().is_err());
        assert!(spec("a/**").check().is_err());
        assert!(spec("{a,b}.db").check().is_err());
        assert!(spec("sessions/*/x.jsonl").check().is_ok());
    }

    #[test]
    fn read_outside_toml_jsonc_text_and_1mb_cap() {
        let dir = scratch("outside");
        let toml = dir.join("config.toml");
        fs::write(
            &toml,
            r#"# 머리 주석 = 무시
model = "gpt-5"   # 끝 주석
[model_providers.ollama]
base_url = "http://localhost:11434/v1"
[ model_providers."my.proxy" ]  # 섹션 주석
base_url = 'https://p.example/v1'
retries = 3
ratio = 0.5
on = true
list = [1, 2]
[[profiles]]
name = "x"
"#,
        )
        .unwrap();
        assert_eq!(
            read_outside(&toml),
            Some(json!({"model": "gpt-5", "model_providers": {
                "ollama": {"base_url": "http://localhost:11434/v1"},
                "my.proxy": {"base_url": "https://p.example/v1", "retries": 3, "ratio": 0.5, "on": true}
            }}))
        );

        let jsonc = dir.join("opencode.json");
        fs::write(
            &jsonc,
            "{\n  // 주석\n  \"a\": [1, 2,],\n  /* 블록 */ \"u\": \"http://x//y /*z*/\",\n}\n",
        )
        .unwrap();
        assert_eq!(
            read_outside(&jsonc),
            Some(json!({"a": [1, 2], "u": "http://x//y /*z*/"}))
        );

        let text = dir.join(".project_root");
        fs::write(&text, "  /work/p1\n").unwrap();
        assert_eq!(read_outside(&text), Some(json!("/work/p1")));

        let big = dir.join("big.json");
        fs::write(&big, vec![b' '; (1 << 20) + 1]).unwrap();
        assert_eq!(read_outside(&big), None, "1 MB를 넘으면 읽지 않는다");
        assert_eq!(read_outside(&dir.join("missing.json")), None);
        let _ = fs::remove_dir_all(dir);
    }
}
