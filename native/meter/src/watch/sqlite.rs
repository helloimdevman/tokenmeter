//! SQLite 읽기(F1, 스펙 5절). 읽기 전용으로 열고, 문장 하나를 자동 커밋으로 끝까지 돌린다.
//! 읽기 경로에 연결하는 것은 Task 2.6이다.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, ErrorCode, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// 변화 감지용 도장. `<db>`와 `<db>-wal`의 크기·mtime(커밋은 `-wal`에만 쌓인다).
#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct DbStamp {
    pub ino: u64,
    pub size: u64,
    pub mtime: f64,
    pub wal_size: u64,
    pub wal_mtime: f64,
}

/// `-shm`은 보지 않는다. 우리 읽기도 그 mtime을 바꾼다. `-wal`이 없으면 0.
pub fn stamp(db: &Path) -> Option<DbStamp> {
    let meta = std::fs::metadata(db).ok()?;
    let mut wal = PathBuf::from(db).into_os_string();
    wal.push("-wal");
    let wal = std::fs::metadata(wal).ok();
    Some(DbStamp {
        ino: meta.ino(),
        size: meta.len(),
        mtime: mtime(&meta),
        wal_size: wal.as_ref().map_or(0, |m| m.len()),
        wal_mtime: wal.as_ref().map_or(0.0, mtime),
    })
}

fn mtime(meta: &std::fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

/// 플래그를 직접 준다(URI 없음). `file:…?mode=ro`는 `#`·`?`·`%`가 든 경로를 잘못 푼다.
/// `immutable=1`은 WAL을 무시하므로 쓰지 않는다.
pub fn open_ro(db: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        db,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(500))?;
    conn.pragma_update(None, "query_only", "ON")?;
    Ok(conn)
}

#[derive(Debug)]
pub enum SqlErr {
    /// WAL 복구·체크포인트 중. 이번 폴을 건너뛴다.
    Busy,
    /// 목록의 쿼리가 하나도 준비되지 않았다(마지막 오류).
    NoQuery(String),
    Other(String),
}

fn err(e: rusqlite::Error) -> SqlErr {
    match e.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy) => SqlErr::Busy,
        _ => SqlErr::Other(e.to_string()),
    }
}

/// 목록에서 처음 준비되는 쿼리를 문장 하나(자동 커밋)로 끝까지 돌린다.
/// cursor가 있으면 ?1 = since − 60초(열 값의 단위로)이고 결과의 최댓값을 돌려준다.
/// since가 없으면(처음 읽기) ?1은 가장 작은 정수다.
pub fn run(
    conn: &Connection,
    queries: &[String],
    cursor: Option<&str>,
    since: Option<i64>,
) -> Result<(Vec<Value>, Option<i64>), SqlErr> {
    let mut last = String::from("no query");
    let mut stmt = None;
    for q in queries {
        match conn.prepare(q) {
            Ok(s) => {
                stmt = Some(s);
                break;
            }
            Err(e) if e.sqlite_error_code() == Some(ErrorCode::DatabaseBusy) => {
                return Err(SqlErr::Busy)
            }
            Err(e) => last = e.to_string(),
        }
    }
    let mut stmt = stmt.ok_or(SqlErr::NoQuery(last))?;
    let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    // 커서 없는 대체 쿼리(?1 없음)도 받는다.
    let from = since.map_or(i64::MIN, back_60s);
    let mut rows = if cursor.is_some() && stmt.parameter_count() > 0 {
        stmt.query([from])
    } else {
        stmt.query([])
    }
    .map_err(err)?;
    let (mut out, mut max) = (Vec::new(), None::<i64>);
    while let Some(row) = rows.next().map_err(err)? {
        let mut obj = Map::new();
        for (i, name) in names.iter().enumerate() {
            obj.insert(name.clone(), json(row.get_ref(i).map_err(err)?));
        }
        if let Some(n) = cursor.and_then(|c| obj.get(c)).and_then(num) {
            max = Some(max.map_or(n, |m| m.max(n)));
        }
        out.push(Value::Object(obj));
    }
    Ok((out, max))
}

fn num(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64))
}

/// 60초를 커서 열의 단위로 뺀다. 단위 문턱은 F8(watch::time)과 같다:
/// 1e11 미만 초, 1e14 미만 밀리초, 그 이상 마이크로초.
fn back_60s(since: i64) -> i64 {
    let step = match since.unsigned_abs() {
        n if n < 100_000_000_000 => 60,
        n if n < 100_000_000_000_000 => 60_000,
        _ => 60_000_000,
    };
    since.saturating_sub(step)
}

/// BLOB은 UTF-8이면 문자열, 아니면 null(zstd BLOB은 Task 2.13).
fn json(v: ValueRef<'_>) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) => i.into(),
        ValueRef::Real(f) => serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
        ValueRef::Blob(b) => std::str::from_utf8(b).map_or(Value::Null, |s| s.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tm-sqlite-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn q(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn rows(
        db: &Path,
        queries: &[&str],
        cursor: Option<&str>,
        since: Option<i64>,
    ) -> (Vec<Value>, Option<i64>) {
        run(&open_ro(db).unwrap(), &q(queries), cursor, since).unwrap()
    }

    #[test]
    fn opens_paths_with_hash_question_percent() {
        let d = dir("uri").join("a#b?c%");
        std::fs::create_dir_all(&d).unwrap();
        let db = d.join("x.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE t(v); INSERT INTO t VALUES (7);")
            .unwrap();
        assert_eq!(
            rows(&db, &["SELECT v FROM t"], None, None).0,
            vec![json!({"v": 7})]
        );
        let conn = open_ro(&db).unwrap();
        assert!(
            conn.execute("INSERT INTO t VALUES (8)", []).is_err(),
            "읽기 전용"
        );
    }

    #[test]
    fn row_types_map_to_json() {
        let db = dir("types").join("x.db");
        let w = Connection::open(&db).unwrap();
        w.execute_batch("CREATE TABLE t(s TEXT, i INTEGER, r REAL, n, u BLOB, b BLOB)")
            .unwrap();
        w.execute(
            "INSERT INTO t VALUES ('hi', 42, 1.5, NULL, ?1, ?2)",
            rusqlite::params![b"{\"a\":1}".to_vec(), vec![0xffu8, 0x00, 0xfe]],
        )
        .unwrap();
        let (out, max) = rows(&db, &["SELECT * FROM t"], None, None);
        assert_eq!(
            out,
            vec![json!({"s": "hi", "i": 42, "r": 1.5, "n": null, "u": "{\"a\":1}", "b": null})]
        );
        assert_eq!(max, None);
    }

    #[test]
    fn first_preparable_query_wins() {
        let db = dir("first").join("x.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE t(a); INSERT INTO t VALUES (2);")
            .unwrap();
        let (out, _) = rows(
            &db,
            &["SELECT a FROM missing", "SELECT a FROM t", "SELECT 9 AS a"],
            None,
            None,
        );
        assert_eq!(out, vec![json!({"a": 2})]);
    }

    #[test]
    fn all_queries_fail_is_an_error_not_a_panic() {
        let db = dir("fail").join("x.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE t(a)")
            .unwrap();
        let conn = open_ro(&db).unwrap();
        let e = run(&conn, &q(&["SELECT a FROM missing", "not sql"]), None, None).unwrap_err();
        assert!(
            matches!(e, SqlErr::NoQuery(ref m) if !m.is_empty()),
            "{e:?}"
        );
        assert!(matches!(
            run(&conn, &[], None, None),
            Err(SqlErr::NoQuery(_))
        ));
    }

    #[test]
    fn cursor_rereads_last_60_seconds_in_the_column_unit() {
        let db = dir("cursor").join("x.db");
        let w = Connection::open(&db).unwrap();
        w.execute_batch("CREATE TABLE ms(id, updated); CREATE TABLE s(id, updated);")
            .unwrap();
        let ms = 1_790_000_100_000i64;
        for (id, t) in [(1, ms - 60_001), (2, ms - 60_000), (3, ms)] {
            w.execute("INSERT INTO ms VALUES (?1, ?2)", [id, t])
                .unwrap();
        }
        let s = 1_790_000_100i64;
        for (id, t) in [(1, s - 61), (2, s - 60), (3, s + 5)] {
            w.execute("INSERT INTO s VALUES (?1, ?2)", [id, t]).unwrap();
        }
        let sql = |t: &str| format!("SELECT id, updated FROM {t} WHERE updated >= ?1 ORDER BY id");
        let ids = |v: &[Value]| {
            v.iter()
                .map(|r| r["id"].as_i64().unwrap())
                .collect::<Vec<_>>()
        };

        let (out, max) = rows(&db, &[&sql("ms")], Some("updated"), Some(ms));
        assert_eq!((ids(&out), max), (vec![2, 3], Some(ms)));
        let (out, max) = rows(&db, &[&sql("s")], Some("updated"), Some(s));
        assert_eq!((ids(&out), max), (vec![2, 3], Some(s + 5)));
        // 처음 읽기: 커서는 있지만 since가 없으면 전부.
        let (out, max) = rows(&db, &[&sql("s")], Some("updated"), None);
        assert_eq!((ids(&out), max), (vec![1, 2, 3], Some(s + 5)));
        // ?1 없는 대체 쿼리도 커서와 같이 돈다.
        let (out, max) = rows(
            &db,
            &["SELECT id, updated FROM s"],
            Some("updated"),
            Some(s),
        );
        assert_eq!((out.len(), max), (3, Some(s + 5)));
    }

    #[test]
    fn wal_only_rows_are_seen_on_next_poll() {
        let db = dir("wal").join("x.db");
        let w = Connection::open(&db).unwrap();
        w.pragma_update(None, "journal_mode", "WAL").unwrap();
        w.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        w.execute_batch("CREATE TABLE t(a); INSERT INTO t VALUES (1);")
            .unwrap();
        let before = stamp(&db).unwrap();
        assert_eq!(rows(&db, &["SELECT a FROM t"], None, None).0.len(), 1);

        std::thread::sleep(Duration::from_millis(20));
        w.execute("INSERT INTO t VALUES (2)", []).unwrap();
        let after = stamp(&db).unwrap();
        assert_ne!(
            (before.wal_size, before.wal_mtime),
            (after.wal_size, after.wal_mtime),
            "커밋은 -wal에 쌓인다"
        );
        assert_eq!(
            (before.ino, before.size),
            (after.ino, after.size),
            "본 파일은 체크포인트 때만"
        );
        assert_eq!(rows(&db, &["SELECT a FROM t"], None, None).0.len(), 2);
    }

    #[test]
    fn reader_does_not_hold_the_wal() {
        let db = dir("hold").join("x.db");
        let w = Connection::open(&db).unwrap();
        w.pragma_update(None, "journal_mode", "WAL").unwrap();
        w.execute_batch("CREATE TABLE t(a)").unwrap();
        // 연결은 폴 사이에 살아 있어도 읽기 트랜잭션은 문장과 함께 끝나야 한다.
        let r = open_ro(&db).unwrap();
        for i in 0..100 {
            w.execute("INSERT INTO t VALUES (?1)", [i]).unwrap();
            let (out, _) = run(&r, &q(&["SELECT a FROM t"]), None, None).unwrap();
            assert_eq!(out.len(), i as usize + 1);
        }
        let (busy, log, done): (i64, i64, i64) = w
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |x| {
                Ok((x.get(0)?, x.get(1)?, x.get(2)?))
            })
            .unwrap();
        assert_eq!((busy, done), (0, log), "모든 프레임을 옮긴다");
        drop(r);
    }

    #[test]
    fn busy_is_reported_as_busy() {
        let db = dir("busy").join("x.db");
        let w = Connection::open(&db).unwrap();
        w.execute_batch("CREATE TABLE t(a); INSERT INTO t VALUES (1); BEGIN EXCLUSIVE;")
            .unwrap();
        let e = run(&open_ro(&db).unwrap(), &q(&["SELECT a FROM t"]), None, None).unwrap_err();
        assert!(matches!(e, SqlErr::Busy), "{e:?}");
        w.execute_batch("COMMIT").unwrap();
    }

    #[test]
    fn stamp_is_none_without_db_and_zero_without_wal() {
        let d = dir("stamp");
        assert!(stamp(&d.join("none.db")).is_none());
        let db = d.join("x.db");
        Connection::open(&db)
            .unwrap()
            .execute_batch("CREATE TABLE t(a)")
            .unwrap();
        let s = stamp(&db).unwrap();
        assert!(s.ino > 0 && s.size > 0 && s.mtime > 0.0);
        assert_eq!((s.wal_size, s.wal_mtime), (0, 0.0));
    }

    #[test]
    fn back_60s_follows_f8_units() {
        assert_eq!(back_60s(1_790_000_000), 1_789_999_940);
        assert_eq!(back_60s(1_790_000_000_000), 1_789_999_940_000);
        assert_eq!(back_60s(1_790_000_000_000_000), 1_789_999_940_000_000);
        assert_eq!(back_60s(i64::MIN), i64::MIN);
    }
}
