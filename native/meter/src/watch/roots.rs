//! 루트 경로 펼치기(`~`, `$TOKENMETER_HOME`, `$VAR`). Task 1.3이 넓힌다.

use std::path::PathBuf;
use tokenmeter_hook::data_dir;

pub(crate) fn expand_home(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("$TOKENMETER_HOME") {
        let rest = rest.trim_start_matches(['/', '\\']);
        return if rest.is_empty() {
            data_dir()
        } else {
            data_dir().join(rest)
        };
    }
    let expanded = expand_vars(raw);
    if let Some(rest) = expanded.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(expanded)
}

fn expand_vars(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let (start, end, next) = if chars.get(i + 1) == Some(&'{') {
            let Some(close) = chars[i + 2..].iter().position(|c| *c == '}') else {
                out.push('$');
                i += 1;
                continue;
            };
            (i + 2, i + 2 + close, i + 3 + close)
        } else {
            let mut end = i + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            (i + 1, end, end)
        };
        if start == end {
            out.push('$');
        } else {
            let name: String = chars[start..end].iter().collect();
            out.push_str(&std::env::var(name).unwrap_or_default());
        }
        i = next;
    }
    out
}

pub(super) fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}
