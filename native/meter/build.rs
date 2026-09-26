//! adapters/*.yaml을 이름순으로 모아 $OUT_DIR/adapters.rs의 ADAPTERS 표를 만든다(스펙 1절).
use std::{env, fs, path::Path};

fn main() {
    let dir = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("adapters");
    // 디렉터리를 가리키면 Cargo가 안의 파일 변화를 모두 본다. 파일을 더하거나 지우면 다시 돈다.
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut ids: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".yaml").map(str::to_string))
        .collect();
    ids.sort();
    let mut out = String::from("pub const ADAPTERS: &[(&str, &str)] = &[\n");
    for id in &ids {
        let ok = !id.is_empty()
            && id.len() <= 32
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        assert!(ok, "adapter file name must be [a-z0-9-]{{1,32}}: {id}");
        let path = dir.join(format!("{id}.yaml")).display().to_string();
        out += &format!("    ({id:?}, include_str!({path:?})),\n");
    }
    out += "];\n";
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("adapters.rs"),
        out,
    )
    .unwrap();
}
