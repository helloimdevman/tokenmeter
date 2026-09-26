//! 서비스 스펙: YAML 로딩, 사용자 덮어쓰기 병합, 식 자리 파싱과 검증, 설정 읽기.

use super::cond::{self, Cond};
use super::expr::{legacy_path, Pick};
use super::reader::TOKEN_FIELDS;
use super::roots::{self, check_pattern, check_root, home_dir, RootsFrom, Vars};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tokenmeter_hook::data_dir;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ServiceSpec {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub roots: Vec<String>,
    #[serde(default = "default_patterns")]
    pub patterns: Vec<String>,
    /// 루트 기준 glob. 맞는 파일은 읽지 않는다(F10).
    #[serde(default)]
    pub exclude: Vec<String>,
    /// 다른 앱의 레지스트리에서 읽는 루트(F10).
    #[serde(default)]
    pub roots_from: Vec<RootsFromSpec>,
    #[serde(default = "default_format")]
    pub format: String,
    /// 식 자리(스펙 2.0)는 YAML 그대로 두고 `Compiled::new`가 파싱한다. null은 자리가 없는 것.
    #[serde(default)]
    pub match_fields: serde_yaml::Mapping,
    #[serde(default = "default_mode")]
    pub mode: String,
    /// `format: sqlite`의 쿼리. 목록이면 앞에서부터 처음 준비되는 것(F1, 스펙 5절).
    #[serde(default)]
    pub query: Vec<String>,
    /// 숫자 결과 열 이름. `?1`에 지난번 최댓값 − 60초를 넣는다. 비면 커서 없이 전체.
    #[serde(default)]
    pub cursor: String,
    #[serde(default)]
    pub key: serde_yaml::Value,
    /// input에 들어 있어서 뺄 칸 이름(F5).
    #[serde(default)]
    pub input_includes: Vec<String>,
    #[serde(default)]
    pub fields: HashMap<String, serde_yaml::Value>,
    /// 이름 → 식, 또는 `{path: 식, when: match}`(F6).
    #[serde(default)]
    pub context: HashMap<String, serde_yaml::Value>,
    /// 레코드를 원소마다 펼치는 식(F4).
    #[serde(default)]
    pub each: serde_yaml::Value,
    /// 이름 → 데이터 파일 기준 상대 경로 또는 F10 경로 틀(F6).
    #[serde(default)]
    pub sidecars: HashMap<String, String>,
    #[serde(default)]
    pub ctx_tokens: serde_yaml::Value,
    #[serde(default)]
    pub ctx_window: serde_yaml::Value,
    #[serde(default)]
    pub subagent: serde_yaml::Value,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub plan: String,
    #[serde(default)]
    pub plan_probe: serde_yaml::Value,
    #[serde(default)]
    pub endpoint_probe: serde_yaml::Value,
    #[serde(default)]
    pub live_chars: serde_yaml::Value,
    #[serde(default)]
    pub duration_ms: serde_yaml::Value,
    #[serde(default)]
    pub cost_usd: serde_yaml::Value,
    #[serde(default)]
    pub rebase_on: serde_yaml::Value,
    /// 레코드 시각(F8). 없으면 읽은 시각.
    #[serde(default)]
    pub timestamp: serde_yaml::Value,
    /// 실제 로그로 맞춰 본 어댑터인지. false면 `doctor`가 "검증 안 됨"으로 보인다.
    #[serde(default = "default_true")]
    pub verified: bool,
    #[serde(default)]
    pub install: InstallSpec,
    /// 읽기 단위(F11). 항목마다 서비스 수준 값 위에 깊은 병합한 스펙이다. 비면 서비스 자체가 소스 하나다.
    #[serde(default)]
    pub sources: Vec<ServiceSpec>,
}

/// `roots_from` 항목 하나. 식 자리는 YAML 그대로 두고 `Compiled::new`가 파싱한다.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RootsFromSpec {
    #[serde(default)]
    pub file: String,
    #[serde(default)]
    pub each: serde_yaml::Value,
    #[serde(default)]
    pub path: serde_yaml::Value,
    #[serde(default)]
    pub base: serde_yaml::Value,
    #[serde(default)]
    pub patterns: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct InstallSpec {
    #[serde(default)]
    pub target: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub events: Vec<String>,
}

fn default_patterns() -> Vec<String> {
    vec!["**/*.jsonl".into()]
}
fn default_format() -> String {
    "jsonl".into()
}
fn default_mode() -> String {
    "delta".into()
}
fn default_true() -> bool {
    true
}

/// 서비스의 식 자리를 한 번 파싱한 것(F3, F7). 로더가 검증에 쓰고 리더가 레코드마다 평가한다.
#[derive(Default)]
pub struct Compiled {
    /// `TOKEN_FIELDS` 순서.
    pub fields: [Option<Pick>; 4],
    /// `input_includes`의 `TOKEN_FIELDS` 번호.
    pub input_includes: Vec<usize>,
    pub context: Vec<Ctx>,
    pub each: Option<Pick>,
    /// (이름, 경로 틀). 틀 문법은 로딩 때 본다.
    pub sidecars: Vec<(String, String)>,
    pub key: Option<Pick>,
    pub conds: Vec<Cond>,
    pub ctx_tokens: Option<Pick>,
    pub ctx_window: Option<Pick>,
    pub duration_ms: Option<Pick>,
    pub cost_usd: Option<Pick>,
    pub rebase_on: Option<Pick>,
    pub timestamp: Option<Pick>,
    pub subagent: Option<Pick>,
    pub live_chars: Option<Pick>,
    pub plan_key: Option<Pick>,
    pub endpoint_key: Option<Pick>,
    pub exclude: Vec<glob::Pattern>,
    pub roots_from: Vec<RootsFrom>,
}

/// 문맥 자리 하나(F6). `when`이 있으면 그 조건에 맞는 레코드에서만 배운다.
pub struct Ctx {
    pub name: String,
    pub pick: Pick,
    pub when: Option<Vec<Cond>>,
}

impl Compiled {
    /// 틀린 자리가 하나라도 있으면 "자리: 이유"(1절 검증, 그 서비스만 빠진다).
    pub fn new(spec: &ServiceSpec) -> Result<Self, String> {
        const NONE: serde_yaml::Value = serde_yaml::Value::Null;
        // null·빈 글자는 자리가 없는 것(`cwd: null`로 기본 어댑터의 자리를 지운다)
        let site = |name: &str, v: &serde_yaml::Value| {
            if v.is_null() || v.as_str() == Some("") {
                return Ok(None);
            }
            Pick::from_yaml(v)
                .map(Some)
                .map_err(|e| format!("{name}: {e}"))
        };
        let probe_key = |probe: &str, v: &serde_yaml::Value| {
            site(
                &format!("{probe}.key"),
                yaml_at(v, &["key"]).unwrap_or(&NONE),
            )
        };
        let mut fields: [Option<Pick>; 4] = Default::default();
        for (slot, name) in fields.iter_mut().zip(TOKEN_FIELDS) {
            *slot = site(
                &format!("fields.{name}"),
                spec.fields.get(name).unwrap_or(&NONE),
            )?;
        }
        let mut input_includes = Vec::new();
        for name in &spec.input_includes {
            match TOKEN_FIELDS[1..3].iter().position(|f| *f == name.as_str()) {
                Some(i) => input_includes.push(i + 1),
                None => {
                    return Err(crate::l10n!(
                        "input_includes: {name:?} is not cache_read or cache_write",
                        "input_includes: {name:?}는 cache_read나 cache_write가 아닙니다"
                    ))
                }
            }
        }
        let mut context = Vec::new();
        for (name, v) in &spec.context {
            let at = format!("context.{name}");
            let (path, when) = match v.as_mapping() {
                Some(m) => {
                    if let Some(k) = m.keys().map(yaml_scalar).find(|k| k != "path" && k != "when") {
                        return Err(crate::l10n!(
                            "{at}: unknown key {k} (path, when)",
                            "{at}: 모르는 키 {k}(path, when)"
                        ));
                    }
                    let when = match m.get("when") {
                        None => None,
                        Some(serde_yaml::Value::Mapping(w)) => {
                            Some(cond::parse(w, false).map_err(|e| format!("{at}.when: {e}"))?)
                        }
                        Some(w) => {
                            return Err(crate::l10n!(
                                "{at}.when: expected conditions like {{a: b}}, got {w:?}",
                                "{at}.when: {{a: b}} 같은 조건이어야 하는데 {w:?}가 왔습니다"
                            ))
                        }
                    };
                    let path = site(&format!("{at}.path"), m.get("path").unwrap_or(&NONE))?
                        .ok_or_else(|| crate::l10n!("{at}.path: required", "{at}.path: 필요합니다"))?;
                    (Some(path), when)
                }
                None => (site(&at, v)?, None),
            };
            if let Some(pick) = path {
                context.push(Ctx { name: name.clone(), pick, when });
            }
        }
        let mut sidecars = Vec::new();
        for (name, t) in &spec.sidecars {
            let vars = Vars { root: None, ctx: &|_| None };
            roots::expand(t, &vars).map_err(|e| format!("sidecars.{name}: {e}"))?;
            sidecars.push((name.clone(), t.clone()));
        }
        // 루트 틀(F10): 와일드카드·중괄호와 틀 문법 오류는 로딩 때 막는다
        let template = |t: &str| {
            let vars = Vars { root: None, ctx: &|_| None };
            check_root(t).and_then(|_| roots::expand(t, &vars).map(drop))
        };
        for r in &spec.roots {
            template(r).map_err(|e| format!("roots: {e}"))?;
        }
        for p in &spec.patterns {
            check_pattern(p).map_err(|e| format!("patterns: {e}"))?;
        }
        let mut exclude = Vec::new();
        for p in &spec.exclude {
            check_pattern(p)
                .and_then(|_| glob::Pattern::new(p).map_err(|e| e.to_string()))
                .map(|p| exclude.push(p))
                .map_err(|e| format!("exclude: {e}"))?;
        }
        let mut roots_from = Vec::new();
        for (i, r) in spec.roots_from.iter().enumerate() {
            let at = format!("roots_from[{i}]");
            let rf = RootsFrom {
                file: r.file.clone(),
                each: site(&format!("{at}.each"), &r.each)?,
                path: site(&format!("{at}.path"), &r.path)?
                    .ok_or_else(|| crate::l10n!("{at}.path: required", "{at}.path: 필요합니다"))?,
                base: site(&format!("{at}.base"), &r.base)?,
                patterns: r.patterns.clone(),
            };
            template(&rf.file)
                .and_then(|_| rf.check())
                .map_err(|e| format!("{at}: {e}"))?;
            roots_from.push(rf);
        }
        Ok(Self {
            fields,
            input_includes,
            context,
            each: site("each", &spec.each)?,
            sidecars,
            key: site("key", &spec.key)?,
            conds: cond::parse(&spec.match_fields, false).map_err(|e| format!("match: {e}"))?,
            ctx_tokens: site("ctx_tokens", &spec.ctx_tokens)?,
            ctx_window: site("ctx_window", &spec.ctx_window)?,
            duration_ms: site("duration_ms", &spec.duration_ms)?,
            cost_usd: site("cost_usd", &spec.cost_usd)?,
            rebase_on: site("rebase_on", &spec.rebase_on)?,
            timestamp: site("timestamp", &spec.timestamp)?,
            subagent: site("subagent", &spec.subagent)?,
            live_chars: site("live_chars", &spec.live_chars)?,
            plan_key: probe_key("plan_probe", &spec.plan_probe)?,
            endpoint_key: probe_key("endpoint_probe", &spec.endpoint_probe)?,
            exclude,
            roots_from,
        })
    }
}

/// 사용자 덮어쓰기 블록의 옛 형식을 새 형식으로(스펙 1절): 식 자리의 `a.0.b` → `a[0].b`,
/// match의 `X: null` → `{$exists: false}`, 프로브 키의 `{vendor}` → `[$ctx.vendor]`,
/// `input_includes_cache: true|false` → `input_includes: [cache_read]|[]`, 서비스 수준 `endpoint` → `endpoint_probe.default`.
/// 새 형식은 바꾸지 않는다. 기본 어댑터는 이것이 아무것도 바꾸지 않아야 한다(테스트).
pub(super) fn upgrade_legacy(block: &mut serde_yaml::Value) {
    use serde_yaml::Value as Y;
    fn paths(v: &mut Y) {
        match v {
            Y::String(s) => *s = legacy_path(s),
            Y::Sequence(items) => items.iter_mut().for_each(paths),
            _ => {}
        }
    }
    fn conds(m: &mut serde_yaml::Mapping) {
        *m = std::mem::take(m)
            .into_iter()
            .map(|(k, mut v)| {
                if k.as_str() == Some("$any") {
                    for group in v.as_sequence_mut().into_iter().flatten() {
                        if let Y::Mapping(g) = group {
                            conds(g);
                        }
                    }
                    return (k, v);
                }
                if v.is_null() {
                    v = serde_yaml::Mapping::from_iter([("$exists".into(), false.into())]).into();
                }
                match k {
                    Y::String(k) => (Y::String(legacy_path(&k)), v),
                    k => (k, v),
                }
            })
            .collect();
    }
    let Some(block) = block.as_mapping_mut() else {
        return;
    };
    for (name, v) in block.iter_mut() {
        match name.as_str().unwrap_or_default() {
            "key" | "ctx_tokens" | "ctx_window" | "duration_ms" | "subagent" | "live_chars"
            | "timestamp" => {
                paths(v)
            }
            "fields" | "context" => v
                .as_mapping_mut()
                .into_iter()
                .flat_map(|m| m.values_mut())
                .for_each(paths),
            "match" => v.as_mapping_mut().into_iter().for_each(conds),
            "sources" => v.as_sequence_mut().into_iter().flatten().for_each(upgrade_legacy),
            "plan_probe" | "endpoint_probe" => {
                if let Some(key) = v.get_mut("key") {
                    if let Y::String(s) = key {
                        *s = s
                            .replace(".{vendor}", "[$ctx.vendor]")
                            .replace("{vendor}", "[$ctx.vendor]");
                    }
                    paths(key);
                }
            }
            _ => {}
        }
    }
    if let Some(old) = block.remove("input_includes_cache") {
        let parts = if old.as_bool() == Some(true) {
            vec!["cache_read".into()]
        } else {
            vec![]
        };
        if !block.contains_key("input_includes") {
            block.insert("input_includes".into(), Y::Sequence(parts));
        }
    }
    if let Some(old) = block.remove("endpoint") {
        let probe = block
            .entry("endpoint_probe".into())
            .or_insert_with(|| Y::Mapping(Default::default()));
        if let Some(probe) = probe.as_mapping_mut() {
            probe.entry("default".into()).or_insert(old);
        }
    }
}

pub(super) fn yaml_scalar(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::Null => "null".into(),
        serde_yaml::Value::Bool(value) => value.to_string(),
        serde_yaml::Value::Number(value) => value.to_string(),
        serde_yaml::Value::String(value) => value.clone(),
        value => serde_yaml::to_string(value)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

#[derive(Deserialize, Default)]
struct YamlService {
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    roots: Vec<String>,
    #[serde(default)]
    patterns: Option<Vec<String>>,
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    roots_from: Vec<RootsFromSpec>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default, rename = "match")]
    match_fields: serde_yaml::Mapping,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    query: serde_yaml::Value,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    key: serde_yaml::Value,
    #[serde(default)]
    input_includes: Vec<String>,
    #[serde(default)]
    fields: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    context: HashMap<String, serde_yaml::Value>,
    #[serde(default)]
    each: serde_yaml::Value,
    #[serde(default)]
    sidecars: HashMap<String, String>,
    #[serde(default)]
    ctx_tokens: serde_yaml::Value,
    #[serde(default)]
    ctx_window: serde_yaml::Value,
    #[serde(default)]
    subagent: serde_yaml::Value,
    #[serde(default)]
    default_model: Option<String>,
    #[serde(default)]
    vendor: Option<String>,
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    plan_probe: serde_yaml::Value,
    #[serde(default)]
    endpoint_probe: serde_yaml::Value,
    #[serde(default)]
    live_chars: serde_yaml::Value,
    #[serde(default)]
    duration_ms: serde_yaml::Value,
    #[serde(default)]
    cost_usd: serde_yaml::Value,
    #[serde(default)]
    rebase_on: serde_yaml::Value,
    #[serde(default)]
    timestamp: serde_yaml::Value,
    #[serde(default)]
    verified: Option<bool>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    install: Option<InstallSpec>,
    #[serde(default)]
    sources: Vec<serde_yaml::Mapping>,
}

/// `settings`만 든다. 기본 서비스는 `ADAPTERS`(adapters/*.yaml)에서 온다.
const DEFAULT_YAML: &str = include_str!("../../services.yaml");

include!(concat!(env!("OUT_DIR"), "/adapters.rs"));

/// 소스 항목에 둘 수 있는 키(F11). 서비스 수준에 두면 모든 소스의 기본값이다.
/// 모르는 키는 `LoadReport::warnings`로 간다.
pub const KNOWN_SOURCE_KEYS: &[&str] = &[
    "roots", "patterns", "exclude", "roots_from", "format", "query", "cursor", "match", "mode", "key", "input_includes", "fields",
    "context", "each", "sidecars", "ctx_tokens", "ctx_window", "subagent", "duration_ms", "cost_usd", "rebase_on", "timestamp",
];

/// 서비스 수준에만 두는 키(F11). 소스 항목에 있으면 그 서비스가 빠진다.
pub const SERVICE_ONLY_KEYS: &[&str] = &[
    "enabled", "label", "default_model", "vendor", "plan", "plan_probe", "endpoint_probe", "live_chars", "install", "sources", "verified",
];

/// 로딩에서 빠진 서비스(id, 이유)와 모르는 키 경고. 데몬 로그와 `doctor`가 보인다.
/// 겹치는 루트는 `overlaps`의 (서비스, 루트 번호) 쌍, `roots_from`은 버린 루트 수만 둔다(1절 개인정보).
#[derive(Debug, Default)]
pub struct LoadReport {
    pub skipped: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub overlaps: Vec<[(String, usize); 2]>,
    pub roots_from_dropped: usize,
}

/// 기본 어댑터만(사용자 덮어쓰기 없이), 켜진 것만.
pub fn default_specs() -> Vec<ServiceSpec> {
    let mut specs = load_specs(&builtin_services()).0;
    specs.retain(|s| s.enabled);
    specs
}

/// 패키지에 들어 있는 서비스인지(켜짐과 상관없이). 사용자가 추가한 서비스 이름은 업로드하지 않는다.
pub fn is_builtin_service(name: &str) -> bool {
    ADAPTERS.iter().any(|(id, _)| *id == name)
}

/// 사용자 덮어쓰기까지 합친 설정을 읽을 때 빠진 서비스와 경고.
pub fn load_report() -> LoadReport {
    let (specs, mut report) = load_specs(&load_merged_yaml());
    report.overlaps = overlaps(&specs);
    let vars = Vars { root: None, ctx: &|_| None };
    report.roots_from_dropped = specs
        .iter()
        .flat_map(source_views)
        .flat_map(|s| Compiled::new(s).map(|x| x.roots_from).unwrap_or_default())
        .map(|rf| roots::roots_from(&rf, &vars).1)
        .sum();
    report
}

/// 서비스의 읽기 단위(F11). 소스가 없으면 서비스 자체가 하나다.
fn source_views(spec: &ServiceSpec) -> &[ServiceSpec] {
    if spec.sources.is_empty() {
        std::slice::from_ref(spec)
    } else {
        &spec.sources
    }
}

/// 기본 어댑터 `id`가 글자 그대로(변수 없이) 적은 루트인지. `doctor --json`은 이런 루트만
/// `short_path`로 보이고 나머지는 서비스 id와 번호로 가리킨다(1절 개인정보).
pub fn is_static_builtin_root(id: &str, template: &str) -> bool {
    !template.contains('$')
        && load_specs(&builtin_services())
            .0
            .iter()
            .any(|s| s.name == id && root_templates(s).contains(&template))
}

/// 서비스의 루트 틀. 소스들의 `roots`를 순서대로 합친다. 번호가 `doctor`의 루트 번호다.
pub fn root_templates(spec: &ServiceSpec) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for r in source_views(spec).iter().flat_map(|s| &s.roots) {
        if !out.contains(&r.as_str()) {
            out.push(r);
        }
    }
    out
}

/// 서비스 사이에 겹치는 루트(같거나 한쪽이 다른 쪽의 조상). 서비스 안은 리더가 합친다(F10).
/// ponytail: 서비스 수 × 루트 수의 제곱. 어댑터가 수백 개가 되면 정렬한 경로로 바꾼다.
pub fn overlaps(specs: &[ServiceSpec]) -> Vec<[(String, usize); 2]> {
    let vars = Vars { root: None, ctx: &|_| None };
    let canon = |p: PathBuf| fs::canonicalize(&p).unwrap_or(p);
    let roots: Vec<(&str, usize, Vec<PathBuf>)> = specs
        .iter()
        .flat_map(|s| {
            root_templates(s).into_iter().enumerate().map(|(i, t)| {
                let paths = roots::expand(t, &vars).ok().flatten().unwrap_or_default();
                (s.name.as_str(), i, paths.into_iter().map(canon).collect())
            })
        })
        .collect();
    let meet = |a: &[PathBuf], b: &[PathBuf]| {
        a.iter().any(|x| b.iter().any(|y| x.starts_with(y) || y.starts_with(x)))
    };
    let mut out = Vec::new();
    for (n, (sa, ia, pa)) in roots.iter().enumerate() {
        for (sb, ib, pb) in &roots[n + 1..] {
            if sa != sb && meet(pa, pb) {
                out.push([(sa.to_string(), *ia), (sb.to_string(), *ib)]);
            }
        }
    }
    out
}

/// `{services: {<id>: <adapters/id.yaml>}}`. 깨진 파일은 빈 블록이 되어 로딩에서 빠진다.
fn builtin_services() -> serde_yaml::Value {
    let services: serde_yaml::Mapping = ADAPTERS
        .iter()
        .map(|(id, text)| ((*id).into(), serde_yaml::from_str(text).unwrap_or_default()))
        .collect();
    let mut root = serde_yaml::Mapping::new();
    root.insert("services".into(), serde_yaml::Value::Mapping(services));
    serde_yaml::Value::Mapping(root)
}

#[derive(Clone, Debug)]
pub struct RuntimeSettings {
    pub enabled: bool,
    pub overlay_auto: bool,
    pub poll_seconds: f64,
    pub idle_exit_minutes: f64,
    pub live_ttl_hours: f64,
    pub session_history: usize,
    pub full_scale: f64,
    pub attention_notify: bool,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            overlay_auto: true,
            poll_seconds: 2.0,
            idle_exit_minutes: 30.0,
            live_ttl_hours: 6.0,
            session_history: 500,
            full_scale: 3000.0,
            attention_notify: true,
        }
    }
}

pub struct RuntimeConfig {
    pub specs: Vec<ServiceSpec>,
    pub settings: RuntimeSettings,
}

pub fn load_merged_yaml() -> serde_yaml::Value {
    let mut raw = serde_yaml::from_str::<serde_yaml::Value>(DEFAULT_YAML)
        .unwrap_or(serde_yaml::Value::Mapping(Default::default()));
    deep_merge_yaml(&mut raw, builtin_services());
    let user_path = config_dir().join("services.yaml");
    if let Ok(text) = fs::read_to_string(user_path) {
        if let Ok(mut user) = serde_yaml::from_str::<serde_yaml::Value>(&text) {
            if let Some(services) = user.get_mut("services").and_then(|s| s.as_mapping_mut()) {
                services.values_mut().for_each(upgrade_legacy);
            }
            deep_merge_yaml(&mut raw, user);
        }
    }
    raw
}

pub fn setting_str(path: &[&str]) -> String {
    yaml_at(&load_merged_yaml(), path)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

pub fn setting_f64(path: &[&str], default: f64) -> f64 {
    yaml_f64(&load_merged_yaml(), path).unwrap_or(default)
}

pub fn setting_map(path: &[&str]) -> HashMap<String, String> {
    let Some(serde_yaml::Value::Mapping(map)) = yaml_at(&load_merged_yaml(), path).cloned() else {
        return HashMap::new();
    };
    map.into_iter()
        .filter_map(|(k, v)| {
            let key = match k {
                serde_yaml::Value::String(s) => s,
                other => yaml_scalar(&other),
            };
            let val = match v {
                serde_yaml::Value::String(s) => s,
                other => yaml_scalar(&other),
            };
            Some((key, val))
        })
        .collect()
}

pub fn setting_list(path: &[&str]) -> Vec<String> {
    yaml_at(&load_merged_yaml(), path)
        .and_then(|v| v.as_sequence())
        .map(|rows| {
            rows.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

pub fn load_all_specs() -> Vec<ServiceSpec> {
    load_specs(&load_merged_yaml()).0
}

pub fn load_runtime_config() -> RuntimeConfig {
    let raw = load_merged_yaml();
    let mut specs = load_specs(&raw).0;
    specs.retain(|s| s.enabled);
    let mut settings = RuntimeSettings {
        poll_seconds: yaml_f64(&raw, &["settings", "poll_seconds"])
            .unwrap_or(2.0)
            .max(0.2),
        idle_exit_minutes: yaml_f64(&raw, &["settings", "idle_exit_minutes"])
            .unwrap_or(30.0)
            .max(0.0),
        live_ttl_hours: yaml_f64(&raw, &["settings", "live_ttl_hours"])
            .unwrap_or(6.0)
            .max(0.0),
        session_history: yaml_f64(&raw, &["settings", "session_history"])
            .unwrap_or(500.0)
            .max(20.0) as usize,
        full_scale: yaml_f64(&raw, &["settings", "overlay", "full_scale"])
            .unwrap_or(3000.0)
            .max(1.0),
        attention_notify: yaml_bool(&raw, &["settings", "attention_notify"]).unwrap_or_else(|| {
            yaml_f64(&raw, &["settings", "idle_notify_seconds"]).unwrap_or(0.0) != 0.0
        }),
        ..Default::default()
    };

    if let Ok(text) = fs::read_to_string(data_dir().join("toggle.json")) {
        if let Ok(toggle) = serde_json::from_str::<Value>(&text) {
            settings.enabled = toggle.get("enabled").and_then(Value::as_bool) != Some(false);
            settings.overlay_auto = toggle.get("overlay").and_then(Value::as_bool) != Some(false);
            if let Some(book) = toggle.get("services").and_then(Value::as_object) {
                specs.retain(|spec| book.get(&spec.name).and_then(Value::as_bool) != Some(false));
            }
        }
    }
    RuntimeConfig { specs, settings }
}

pub fn specs_from_yaml(text: &str) -> Vec<ServiceSpec> {
    specs_from_yaml_opts(text, false)
}

/// `services:` 아래 블록을 읽기만 한다(타입이 틀린 블록은 빠진다). `format`·`mode` 검증은 로더 몫.
pub fn specs_from_yaml_opts(text: &str, include_disabled: bool) -> Vec<ServiceSpec> {
    let raw = serde_yaml::from_str(text).unwrap_or_default();
    let mut specs = read_services(&raw, &mut LoadReport::default());
    specs.retain(|s| include_disabled || s.enabled);
    specs
}

/// 합친 설정의 서비스를 모두(꺼진 것 포함) 읽고 검증한다(B5: 틀린 블록 하나는 그 서비스만 빠진다).
fn load_specs(raw: &serde_yaml::Value) -> (Vec<ServiceSpec>, LoadReport) {
    let mut report = LoadReport::default();
    let mut specs = read_services(raw, &mut report);
    specs.retain(|s| {
        // 소스가 있으면 서비스 수준은 식 파싱만 본다(형식·키 규칙은 병합한 소스마다)
        let why = if s.sources.is_empty() {
            check_source(s).err()
        } else {
            Compiled::new(s).err().or_else(|| {
                s.sources.iter().enumerate().find_map(|(i, src)| {
                    check_source(src).err().map(|e| format!("sources[{i}].{e}"))
                })
            })
        };
        let Some(why) = why else { return true };
        report.skipped.push((s.name.clone(), why));
        false
    });
    (specs, report)
}

/// 읽기 단위 하나의 검증(1절). 틀리면 "자리: 이유".
fn check_source(s: &ServiceSpec) -> Result<(), String> {
    if !["jsonl", "json", "sqlite"].contains(&s.format.as_str()) {
        return Err(format!("format: unknown value {:?} (jsonl, json, sqlite)", s.format));
    }
    if s.format == "sqlite" && s.query.is_empty() {
        return Err(crate::l10n!(
            "query: required when format is sqlite",
            "query: format이 sqlite이면 필요합니다"
        ));
    }
    if !["delta", "cumulative"].contains(&s.mode.as_str()) {
        return Err(format!("mode: unknown value {:?} (delta, cumulative)", s.mode));
    }
    // 파일 안 위치로는 레코드를 가를 수 없다(F2). SQLite는 커서가 경계 행을 다시 읽는다(F1).
    let x = Compiled::new(s)?;
    if x.key.is_none() && s.format != "jsonl" {
        return Err(crate::l10n!(
            "key: required when format is {}",
            "key: format이 {}이면 필요합니다",
            s.format
        ));
    }
    if x.each.is_some() && x.key.is_none() {
        return Err(crate::l10n!("key: required with each", "key: each가 있으면 필요합니다"));
    }
    // 원소들이 기준값 하나를 번갈아 덮어쓰지 않게(F4)
    // ponytail: 식 글자에서 찾는다. 따옴표 키 안의 "$key"도 통과시키지만 그런 키를 쓰는 로그는 없다.
    let per_elem = |v: &serde_yaml::Value| serde_yaml::to_string(v).is_ok_and(|t| t.contains("$key") || t.contains("$index"));
    if x.each.is_some() && s.mode == "cumulative" && !per_elem(&s.key) {
        return Err(crate::l10n!(
            "key: with each and mode cumulative, the key needs $key or $index",
            "key: each와 mode cumulative를 같이 쓰면 키에 $key나 $index가 있어야 합니다"
        ));
    }
    Ok(())
}

/// 서비스 블록마다 따로 `YamlService`로 읽는다. 실패하면 `skipped`, 모르는 키는 `warnings`.
fn read_services(raw: &serde_yaml::Value, report: &mut LoadReport) -> Vec<ServiceSpec> {
    let Some(services) = yaml_at(raw, &["services"]).and_then(|v| v.as_mapping()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, block) in services {
        let name = yaml_scalar(name);
        for key in block.as_mapping().into_iter().flat_map(|m| m.keys()) {
            let key = yaml_scalar(key);
            if !KNOWN_SOURCE_KEYS.contains(&key.as_str()) && !SERVICE_ONLY_KEYS.contains(&key.as_str()) {
                report.warnings.push(format!("{name}: unknown key {key}"));
            }
        }
        match read_service(&name, block, report) {
            Ok(spec) => out.push(spec),
            Err(why) => report.skipped.push((name, why)),
        }
    }
    out
}

/// 서비스 블록 하나와 그 소스들. 소스 항목은 `sources`를 뺀 블록 위에 깊은 병합한다(2.0, F11).
fn read_service(name: &str, block: &serde_yaml::Value, report: &mut LoadReport) -> Result<ServiceSpec, String> {
    let (mut spec, entries) = parse_block(name, block)?;
    let mut base = block.clone();
    if let Some(m) = base.as_mapping_mut() {
        m.remove("sources");
    }
    for (i, entry) in entries.into_iter().enumerate() {
        for key in entry.keys().map(yaml_scalar) {
            if SERVICE_ONLY_KEYS.contains(&key.as_str()) {
                return Err(crate::l10n!(
                    "sources[{i}].{key}: only allowed at the service level",
                    "sources[{i}].{key}: 서비스 수준에만 둘 수 있습니다"
                ));
            }
            if !KNOWN_SOURCE_KEYS.contains(&key.as_str()) {
                report.warnings.push(format!("{name}: unknown key sources[{i}].{key}"));
            }
        }
        let mut merged = base.clone();
        deep_merge_yaml(&mut merged, entry.into());
        let (source, _) = parse_block(name, &merged).map_err(|e| format!("sources[{i}].{e}"))?;
        spec.sources.push(source);
    }
    Ok(spec)
}

/// 블록 하나를 `YamlService`로 읽어 스펙과 (병합 전) 소스 항목으로 나눈다.
fn parse_block(name: &str, block: &serde_yaml::Value) -> Result<(ServiceSpec, Vec<serde_yaml::Mapping>), String> {
    // 글로 다시 읽어야 오류에 틀린 키 이름이 붙는다(`from_value`는 경로를 잃는다).
    let text = serde_yaml::to_string(block).unwrap_or_default();
    let raw = serde_yaml::from_str::<YamlService>(&text).map_err(|e| e.to_string())?;
    let bad_query = || crate::l10n!("query: expected a string or a list of strings", "query: 글자나 글자 목록이어야 합니다");
    let query = match raw.query {
        serde_yaml::Value::Null => Vec::new(),
        serde_yaml::Value::String(q) => vec![q],
        serde_yaml::Value::Sequence(items) => items
            .iter()
            .map(|q| q.as_str().map(str::to_string))
            .collect::<Option<_>>()
            .ok_or_else(bad_query)?,
        _ => return Err(bad_query()),
    };
    let name = name.to_string();
    let enabled = raw.enabled != Some(false);
    Ok((
        ServiceSpec {
            name: name.clone(),
            label: raw.label.unwrap_or(name),
            enabled,
            roots: raw.roots,
            patterns: raw.patterns.unwrap_or_else(default_patterns),
            exclude: raw.exclude,
            roots_from: raw.roots_from,
            format: raw.format.unwrap_or_else(default_format),
            match_fields: raw.match_fields,
            mode: raw.mode.unwrap_or_else(default_mode),
            query,
            cursor: raw.cursor.unwrap_or_default(),
            key: raw.key,
            input_includes: raw.input_includes,
            fields: raw.fields,
            context: raw.context,
            each: raw.each,
            sidecars: raw.sidecars,
            ctx_tokens: raw.ctx_tokens,
            ctx_window: raw.ctx_window,
            subagent: raw.subagent,
            default_model: raw.default_model.unwrap_or_default(),
            vendor: raw.vendor.unwrap_or_default(),
            plan: raw.plan.unwrap_or_default(),
            plan_probe: raw.plan_probe,
            endpoint_probe: raw.endpoint_probe,
            live_chars: raw.live_chars,
            duration_ms: raw.duration_ms,
            cost_usd: raw.cost_usd,
            rebase_on: raw.rebase_on,
            timestamp: raw.timestamp,
            verified: raw.verified != Some(false),
            install: raw.install.unwrap_or_default(),
            sources: Vec::new(),
        },
        raw.sources,
    ))
}

fn config_dir() -> PathBuf {
    if let Ok(path) = std::env::var("XDG_CONFIG_HOME") {
        if !path.trim().is_empty() {
            return PathBuf::from(path).join("tokenmeter");
        }
    }
    home_dir().unwrap_or_default().join(".config/tokenmeter")
}

pub(super) fn deep_merge_yaml(base: &mut serde_yaml::Value, over: serde_yaml::Value) {
    match (base, over) {
        (serde_yaml::Value::Mapping(base), serde_yaml::Value::Mapping(over)) => {
            for (key, value) in over {
                if let Some(existing) = base.get_mut(&key) {
                    deep_merge_yaml(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, over) => *base = over,
    }
}

pub(super) fn yaml_at<'a>(value: &'a serde_yaml::Value, path: &[&str]) -> Option<&'a serde_yaml::Value> {
    let mut current = value;
    for part in path {
        current = current
            .as_mapping()?
            .get(serde_yaml::Value::String((*part).into()))?;
    }
    Some(current)
}

pub(super) fn yaml_f64(value: &serde_yaml::Value, path: &[&str]) -> Option<f64> {
    let value = yaml_at(value, path)?;
    value.as_f64().or_else(|| value.as_str()?.parse().ok())
}

fn yaml_bool(value: &serde_yaml::Value, path: &[&str]) -> Option<bool> {
    let value = yaml_at(value, path)?;
    value
        .as_bool()
        .or_else(|| match value.as_str()?.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => Some(true),
            "false" | "0" | "no" => Some(false),
            _ => None,
        })
}

pub(super) fn yaml_string(value: &serde_yaml::Value, key: &str) -> String {
    yaml_at(value, &[key])
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

pub(super) fn yaml_strings(value: &serde_yaml::Value, key: &str) -> Vec<String> {
    yaml_at(value, &[key])
        .and_then(|v| v.as_sequence())
        .map(|rows| {
            rows.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
