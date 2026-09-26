//! 읽기 상태 파일과 커밋 규약(F8, 스펙 4.1·4.2).
//! 서비스마다 `readers/<id>.json`(파일 상태)과 `readers/<id>.keys`(키 장부 덧붙이기 로그)를 둔다.
//! 경로와 키 원문은 남기지 않고 FNV-1a 64비트 해시만 쓴다.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// 이보다 큰 `readers/<id>.json`은 `doctor`가 경고한다(스펙 4.1).
const OVERSIZED: u64 = 4 << 20;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct FileEntry {
    pub ino: u64,
    pub size: u64,
    pub mtime: f64,
    pub off: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<(String, u32)>, // (hex 해시, 길이)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<f64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ctx: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub base: BTreeMap<String, [f64; 6]>, // 스트림 키 해시 hex 또는 ""
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal: Option<(u64, f64)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<i64>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ReaderFile {
    pub v: u32,
    pub seq: u64,
    pub at: f64,
    pub files: BTreeMap<String, FileEntry>, // 키 = 정규화 절대 경로의 FNV-1a hex
    pub est: BTreeMap<String, i64>,
}

/// 파일 항목 키: 정규화한 경로(`//`, 가운데 `.`, 끝 `/` 없앰)의 FNV-1a 64비트 hex.
/// 심볼릭 링크는 풀지 않는다(지워진 파일도 같은 키를 낸다).
pub fn path_key(p: &Path) -> String {
    let norm: PathBuf = p.components().collect();
    format!("{:016x}", fnv1a64(norm.to_string_lossy().as_bytes()))
}

// ponytail: ledger::fnv1a64(Task 1.5, 병렬 레인)와 같은 함수다. W2가 하나로 합친다.
fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

pub struct Store {
    pub dir: PathBuf, // data_dir/readers
}

impl Store {
    fn path(&self, id: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{id}.{ext}"))
    }

    pub fn load(&self, id: &str) -> Option<ReaderFile> {
        serde_json::from_slice(&fs::read(self.path(id, "json")).ok()?).ok()
    }

    /// seq > committed 줄은 버린다(커밋되지 않음). 같은 키는 뒤 줄이 이긴다. 결과는 키 순.
    pub fn keys(&self, id: &str, committed: u64) -> Vec<(u64, [f64; 7])> {
        let Ok(text) = fs::read_to_string(self.path(id, "keys")) else {
            return Vec::new();
        };
        let mut out = BTreeMap::new();
        for (seq, key, vals) in text.lines().filter_map(parse_line) {
            if seq <= committed {
                out.insert(key, vals);
            }
        }
        out.into_iter().collect()
    }

    /// 커밋 1단계: .keys에 seq를 단 줄을 덧붙이고 <id>.next.json을 쓴다(임시 파일 → 이름 바꾸기).
    pub fn stage(&self, id: &str, file: &ReaderFile, keys: &[(u64, [f64; 7])]) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        if !keys.is_empty() {
            // 앞의 빈 줄: 죽을 때 찢긴 끝 줄이 남아도 이번 줄과 붙지 않는다.
            let mut buf = String::from("\n");
            for (key, vals) in keys {
                push_line(&mut buf, file.seq, *key, vals);
            }
            fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.path(id, "keys"))?
                .write_all(buf.as_bytes())?;
        }
        write_atomic(&self.path(id, "next.json"), &serde_json::to_vec(file)?)
    }

    /// 커밋 3단계: next → json.
    pub fn finish(&self, id: &str) -> io::Result<()> {
        fs::rename(self.path(id, "next.json"), self.path(id, "json"))
    }

    /// 뜰 때: next.seq ≤ committed면 finish, 크면 지운다.
    pub fn recover(&self, committed: u64) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".next.json")) else {
                continue;
            };
            let seq = fs::read(entry.path())
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .and_then(|v| v.get("seq")?.as_u64());
            if seq.is_some_and(|s| s <= committed) {
                let _ = self.finish(id);
            } else {
                let _ = fs::remove_file(entry.path());
            }
        }
    }

    /// 살아 있는 키만 새 .keys에 쓰고 이름을 바꾼다(뜰 때, 1시간마다).
    /// 뜰 때는 recover 뒤 keys(committed)로 읽은 것을 넘긴다. 커밋되지 않은 줄이 사라져 seq를 다시 써도 섞이지 않는다.
    pub fn compact(&self, id: &str, live: &[(u64, [f64; 7])], seq: u64) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut buf = String::new();
        for (key, vals) in live {
            push_line(&mut buf, seq, *key, vals);
        }
        write_atomic(&self.path(id, "keys"), buf.as_bytes())
    }

    /// 4 MB 넘는 readers/*.json의 서비스 id(doctor 경고).
    pub fn oversized(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<String> = entries
            .flatten()
            .filter(|e| e.metadata().is_ok_and(|m| m.len() > OVERSIZED))
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let id = name.strip_suffix(".json")?;
                (!id.ends_with(".next")).then(|| id.to_string())
            })
            .collect();
        out.sort();
        out
    }
}

/// `seq\tkeyhex\t[본 시각,input,cache_read,cache_write,output(,cost_usd,duration_ms)]`, 뒤 두 칸이 0이면 뺀다.
fn push_line(buf: &mut String, seq: u64, key: u64, vals: &[f64; 7]) {
    let n = if vals[5] == 0.0 && vals[6] == 0.0 {
        5
    } else {
        7
    };
    let nums: Vec<String> = vals[..n].iter().map(f64::to_string).collect();
    buf.push_str(&format!("{seq}\t{key:016x}\t[{}]\n", nums.join(",")));
}

fn parse_line(line: &str) -> Option<(u64, u64, [f64; 7])> {
    let mut parts = line.split('\t');
    let seq = parts.next()?.parse().ok()?;
    let key = u64::from_str_radix(parts.next()?, 16).ok()?;
    let nums: Vec<f64> = serde_json::from_str(parts.next()?).ok()?;
    if !(5..=7).contains(&nums.len()) {
        return None;
    }
    let mut vals = [0.0; 7];
    vals[..nums.len()].copy_from_slice(&nums);
    Some((seq, key, vals))
}

// ponytail: fsync 없음. 프로세스가 죽는 것(kill -9)은 견디고 전원이 나가는 것은 못 견딘다(state.json 저장과 같다).
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> Store {
        let dir = std::env::temp_dir().join(format!("tm-ckpt-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Store { dir }
    }

    fn file(seq: u64) -> ReaderFile {
        ReaderFile {
            v: 1,
            seq,
            at: 100.0,
            ..Default::default()
        }
    }

    fn vals(output: f64) -> [f64; 7] {
        [1000.0, 1.0, 2.0, 3.0, output, 0.0, 0.0]
    }

    #[test]
    fn recover_finishes_when_state_committed() {
        let s = store("finish");
        s.stage("svc", &file(4), &[]).unwrap();
        s.finish("svc").unwrap();
        let mut f = file(5);
        let entry = FileEntry {
            ino: 7,
            size: 20,
            mtime: 1.5,
            off: 20,
            head: Some(("9f2c61d0a8e4b7c3".into(), 20)),
            first: Some(1.0),
            wal: Some((32768, 2.5)),
            cursor: Some(-3),
            ..Default::default()
        };
        f.files.insert("5b1e0c7a9d2f4e81".into(), entry);
        s.stage("svc", &f, &[]).unwrap();
        s.recover(5);
        let got = s.load("svc").unwrap();
        assert_eq!(got.seq, 5);
        let e = &got.files["5b1e0c7a9d2f4e81"];
        assert_eq!(e.head, Some(("9f2c61d0a8e4b7c3".into(), 20)));
        assert_eq!(
            (e.first, e.wal, e.cursor),
            (Some(1.0), Some((32768, 2.5)), Some(-3))
        );
        assert!(!s.dir.join("svc.next.json").exists());
    }

    #[test]
    fn recover_drops_next_when_not_committed() {
        let s = store("drop");
        s.stage("svc", &file(4), &[]).unwrap();
        s.finish("svc").unwrap();
        s.stage("svc", &file(5), &[]).unwrap();
        s.recover(4);
        assert_eq!(s.load("svc").unwrap().seq, 4);
        assert!(!s.dir.join("svc.next.json").exists());
    }

    #[test]
    fn keys_above_committed_seq_are_ignored() {
        let s = store("above");
        s.stage("svc", &file(1), &[(0xa, vals(10.0))]).unwrap();
        s.stage("svc", &file(2), &[(0xa, vals(20.0)), (0xb, vals(5.0))])
            .unwrap();
        assert_eq!(s.keys("svc", 1), vec![(0xa, vals(10.0))]);
        assert!(s.keys("svc", 0).is_empty());
        assert!(s.keys("none", 9).is_empty());
    }

    #[test]
    fn later_key_line_wins() {
        let s = store("later");
        s.stage("svc", &file(1), &[(0xa, vals(10.0))]).unwrap();
        s.stage("svc", &file(2), &[(0xa, vals(20.0))]).unwrap();
        assert_eq!(s.keys("svc", 2), vec![(0xa, vals(20.0))]);
    }

    #[test]
    fn compaction_keeps_only_live_keys() {
        let s = store("compact");
        let costly = [1790312390.0, 2.0, 60955.0, 2161.0, 813.0, 0.5, 1200.0];
        s.stage("svc", &file(1), &[(0xa, costly), (0xb, vals(1.0))])
            .unwrap();
        s.stage("svc", &file(3), &[(0xc, vals(3.0))]).unwrap(); // 커밋되지 않음
        s.compact(
            "svc",
            &[(0xa41f09c2e7d35b10, vals(813.0)), (0xa, costly)],
            2,
        )
        .unwrap();
        let text = fs::read_to_string(s.dir.join("svc.keys")).unwrap();
        assert_eq!(
            text,
            "2\ta41f09c2e7d35b10\t[1000,1,2,3,813]\n2\t000000000000000a\t[1790312390,2,60955,2161,813,0.5,1200]\n"
        );
        assert_eq!(
            s.keys("svc", u64::MAX),
            vec![(0xa, costly), (0xa41f09c2e7d35b10, vals(813.0))]
        );
    }

    #[test]
    fn torn_last_line_does_not_eat_the_next_commit() {
        let s = store("torn");
        s.stage("svc", &file(1), &[(0xa, vals(1.0))]).unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(s.dir.join("svc.keys"))
            .unwrap()
            .write_all(b"2\t00000000000000")
            .unwrap();
        s.stage("svc", &file(2), &[(0xb, vals(2.0))]).unwrap();
        assert_eq!(s.keys("svc", 2), vec![(0xa, vals(1.0)), (0xb, vals(2.0))]);
    }

    #[test]
    fn paths_are_stored_as_hashes_only() {
        let p = Path::new("/Users/me/.claude/projects/p/s.jsonl");
        assert_eq!(
            path_key(p),
            path_key(Path::new("/Users/me//.claude/./projects/p/s.jsonl"))
        );
        assert_eq!(path_key(p).len(), 16);
        let mut f = file(1);
        f.files.insert(
            path_key(p),
            FileEntry {
                off: 3,
                ..Default::default()
            },
        );
        let s = store("paths");
        s.stage("svc", &f, &[]).unwrap();
        let json = fs::read_to_string(s.dir.join("svc.next.json")).unwrap();
        assert!(!json.contains('/'), "{json}");
    }

    #[test]
    fn cold_entry_serializes_short() {
        let e = FileEntry {
            ino: 77,
            size: 8192,
            mtime: 1789000000.5,
            off: 8192,
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"ino":77,"size":8192,"mtime":1789000000.5,"off":8192}"#
        );
    }

    #[test]
    fn oversized_is_reported() {
        let s = store("big");
        fs::create_dir_all(&s.dir).unwrap();
        for (name, len) in [
            ("big.json", OVERSIZED + 1),
            ("big.next.json", OVERSIZED + 1),
            ("big.keys", OVERSIZED + 1),
            ("ok.json", OVERSIZED),
        ] {
            fs::File::create(s.dir.join(name))
                .unwrap()
                .set_len(len)
                .unwrap();
        }
        assert_eq!(s.oversized(), vec!["big".to_string()]);
        let _ = fs::remove_dir_all(&s.dir);
    }
}
