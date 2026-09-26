//! tests/fixtures/<id>/를 가짜 HOME에 펼쳐 실제 어댑터로 읽고 expected.json과 맞춘다(스펙 13절).
//! 전역 환경을 바꾸므로 테스트는 이 파일의 두 개뿐이고 차례로 돈다.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

#[test]
fn fixtures_match_expected() {
    let only: Vec<String> = std::env::var("TOKENMETER_FIXTURES")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();
    let mut bad = Vec::new();
    for (id, _) in tokenmeter::watch::ADAPTERS {
        if !only.is_empty() && !only.iter().any(|o| o == id) {
            continue;
        }
        let dir = root().join(id);
        let want: serde_json::Value = match std::fs::read_to_string(dir.join("expected.json")) {
            Ok(t) => serde_json::from_str(&t).expect("expected.json"),
            Err(_) => {
                bad.push(format!("{id}: no fixture"));
                continue;
            }
        };
        match tokenmeter::fixture::run(id, None, &dir)
            .and_then(|got| tokenmeter::fixture::compare(&want, &got))
        {
            Ok(()) => {}
            Err(e) => bad.push(format!("{id}: {e}")),
        }
    }
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}

#[test]
fn fixtures_contain_no_personal_data() {
    let bad = tokenmeter::fixture::personal_data(&root());
    assert!(bad.is_empty(), "\n{}", bad.join("\n"));
}
