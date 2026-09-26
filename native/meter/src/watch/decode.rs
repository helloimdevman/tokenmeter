//! zstd 프레임 스트리밍(F12). 파일은 독립 프레임을 이어 붙인 것으로 보고 완결된 프레임만 푼다.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};

use ruzstd::decoding::{BlockDecodingStrategy, FrameDecoder};

pub const MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
/// BLOB 하나가 풀어서 넘으면 버리는 크기(파일 프레임 상한과 같은 TokenTracker 값).
const BLOB_CAP: usize = 128 << 20;

pub struct Frames {
    pub data: Vec<u8>,
    pub next: u64,
    pub skipped: usize,
}

pub fn is_zstd(head: &[u8]) -> bool {
    head.starts_with(&MAGIC)
}

/// `from`(프레임 경계)부터 완결된 프레임만 풀어 이어 붙인다. 잘린 마지막 프레임은 남기고,
/// 새 위치는 마지막 완결 프레임의 끝이다. 풀어서 `frame_cap`을 넘는 프레임은 건너뛰고 센다.
/// `budget`은 프레임 사이에서만 본다: 압축 크기가 `budget`보다 큰 프레임 하나도 멈추지 않고 읽는다.
pub fn read_frames(
    file: &mut File,
    from: u64,
    budget: u64,
    frame_cap: usize,
) -> io::Result<Frames> {
    file.seek(SeekFrom::Start(from))?;
    let (data, used, skipped) = frames(BufReader::new(file), budget, frame_cap);
    Ok(Frames {
        data,
        next: from + used,
        skipped,
    })
}

/// BLOB: zstd면 풀어서, 아니면 그대로. 잘렸거나 상한을 넘은 zstd는 `None`.
pub fn blob(bytes: &[u8]) -> Option<Vec<u8>> {
    if !is_zstd(bytes) {
        return Some(bytes.to_vec());
    }
    let (data, used, skipped) = frames(bytes, u64::MAX, BLOB_CAP);
    (used == bytes.len() as u64 && skipped == 0).then_some(data)
}

/// 완결된 프레임을 차례로 푼다. (풀린 바이트, 읽은 압축 바이트, 건너뛴 프레임 수)
fn frames(mut src: impl Read, budget: u64, cap: usize) -> (Vec<u8>, u64, usize) {
    let mut dec = FrameDecoder::new();
    let (mut data, mut used, mut skipped) = (Vec::new(), 0, 0);
    while used < budget {
        let Some(frame) = one(&mut dec, &mut src, cap) else {
            break;
        };
        used += dec.bytes_read_from_source();
        match frame {
            Some(d) => data.extend(d),
            None => skipped += 1,
        }
    }
    (data, used, skipped)
}

/// 프레임 하나를 블록 단위로 푼다. 잘렸거나 깨졌으면 `None`, 풀어서 `cap`을 넘으면 `Some(None)`.
/// ponytail: skippable 프레임(0x184D2A5?)도 깨진 것으로 보고 그 앞에서 멈춘다. 쓰는 에이전트가 생기면 길이만큼 건너뛴다.
fn one(dec: &mut FrameDecoder, src: &mut impl Read, cap: usize) -> Option<Option<Vec<u8>>> {
    dec.reset(&mut *src).ok()?;
    let mut out = Vec::new();
    let mut over = false;
    loop {
        let done = dec
            .decode_blocks(&mut *src, BlockDecodingStrategy::UptoBlocks(1))
            .ok()?;
        // 넘은 뒤에도 프레임 끝을 알려고 끝까지 풀되 버린다(메모리는 창 크기까지).
        if over {
            dec.collect_to_writer(io::sink())
        } else {
            dec.collect_to_writer(&mut out)
        }
        .ok()?;
        if out.len() > cap {
            over = true;
            out = Vec::new();
        }
        if done {
            return Some((!over).then_some(out));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruzstd::encoding::{compress_to_vec, CompressionLevel};
    use std::fs;

    fn z(s: &str) -> Vec<u8> {
        compress_to_vec(s.as_bytes(), CompressionLevel::Fastest)
    }

    fn file(name: &str, bytes: &[u8]) -> File {
        let p = std::env::temp_dir().join(format!("tm-decode-{}-{name}", std::process::id()));
        fs::write(&p, bytes).unwrap();
        File::open(&p).unwrap()
    }

    const A: &str = "{\"a\":1}\n";
    const B: &str = "{\"b\":2}\n";
    const C: &str = "{\"c\":3,\"pad\":\"cccccccccccccccccccccccccccccccc\"}\n";
    const CAP: usize = 1 << 20;

    #[test]
    fn plain_file_is_not_zstd() {
        assert!(!is_zstd(A.as_bytes()));
        assert!(!is_zstd(&MAGIC[..3]));
        assert!(is_zstd(&z(A)));
    }

    #[test]
    fn concatenated_frames_decode_in_order() {
        let bytes = [z(A), z(B), z(C)].concat();
        let got = read_frames(&mut file("concat", &bytes), 0, u64::MAX, CAP).unwrap();
        assert_eq!(got.data, [A, B, C].concat().as_bytes());
        assert_eq!((got.next, got.skipped), (bytes.len() as u64, 0));
    }

    #[test]
    fn truncated_last_frame_is_deferred() {
        let (a, b, c) = (z(A), z(B), z(C));
        let bytes = [&a[..], &b[..], &c[..c.len() / 2]].concat();
        let got = read_frames(&mut file("trunc", &bytes), 0, u64::MAX, CAP).unwrap();
        assert_eq!(got.data, [A, B].concat().as_bytes());
        assert_eq!(got.next, (a.len() + b.len()) as u64);
    }

    #[test]
    fn resume_from_next_reads_only_new_frames() {
        let (a, b, c) = (z(A), z(B), z(C));
        let half = [&a[..], &b[..], &c[..c.len() / 2]].concat();
        let first = read_frames(&mut file("resume", &half), 0, u64::MAX, CAP).unwrap();
        let full = [a, b, c].concat();
        let got = read_frames(&mut file("resume", &full), first.next, u64::MAX, CAP).unwrap();
        assert_eq!(got.data, C.as_bytes());
        assert_eq!(got.next, full.len() as u64);
    }

    #[test]
    fn oversize_frame_is_skipped_and_counted() {
        // 블록(128 KiB)이 여럿인 프레임이 첫 블록에서 상한을 넘는다.
        let big = C.repeat(10_000);
        let bytes = [z(A), z(&big), z(B)].concat();
        let got = read_frames(&mut file("oversize", &bytes), 0, u64::MAX, 1000).unwrap();
        assert_eq!(got.data, [A, B].concat().as_bytes());
        assert_eq!((got.next, got.skipped), (bytes.len() as u64, 1));
    }

    #[test]
    fn budget_stops_between_frames() {
        let (a, b) = (z(A), z(B));
        let bytes = [&a[..], &b[..]].concat();
        // 프레임 하나보다 작은 예산이어도 그 프레임은 끝까지 읽고 멈춘다.
        let got = read_frames(&mut file("budget", &bytes), 0, 1, CAP).unwrap();
        assert_eq!(got.data, A.as_bytes());
        assert_eq!(got.next, a.len() as u64);
    }

    #[test]
    fn blob_zstd_then_plain() {
        let two = [z(A), z(B)].concat();
        assert_eq!(blob(&two).unwrap(), [A, B].concat().as_bytes());
        assert_eq!(blob(A.as_bytes()).unwrap(), A.as_bytes());
        assert_eq!(blob(&two[..two.len() - 1]), None);
    }
}
