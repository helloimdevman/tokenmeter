//! 레코드 시각 읽기(F8). 문자열은 RFC 3339·ISO 8601(`Z` 없으면 UTC)과 SQLite의
//! `YYYY-MM-DD HH:MM:SS`(UTC), 숫자는 크기로 초·밀리초·마이크로초를 가른다.

use serde_json::Value;

use crate::history::civil_of;

/// 레코드 시각을 유닉스 초로. 못 읽으면 None.
pub fn parse_ts(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => from_number(n.as_f64()?),
        Value::String(s) => match s.parse::<f64>() {
            Ok(n) => from_number(n),
            Err(_) => parse_iso(s),
        },
        _ => None,
    }
}

/// 유닉스 초의 로컬 날짜 `YYYY-MM-DD`.
pub fn local_date(ts: f64) -> String {
    let c = civil_of(ts);
    format!("{:04}-{:02}-{:02}", c.year, c.month, c.day)
}

// 1e11 미만 초, 1e14 미만 밀리초, 그 이상 마이크로초.
fn from_number(n: f64) -> Option<f64> {
    if !n.is_finite() {
        return None;
    }
    Some(if n < 1e11 {
        n
    } else if n < 1e14 {
        n / 1e3
    } else {
        n / 1e6
    })
}

// `YYYY-MM-DD(T| )HH:MM:SS(.frac)?(Z|±HH:MM|±HHMM)?`
fn parse_iso(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    if b.len() < 19
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, se) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut secs = (days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se) as f64;
    let mut i = 19;
    if b.get(i) == Some(&b'.') {
        let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        secs += s[i..i + 1 + n].parse::<f64>().ok()?;
        i += 1 + n;
    }
    let off = match &s[i..] {
        "" | "Z" | "z" => 0,
        o => {
            let sign = match o.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let m = match o.len() {
                6 if o.as_bytes()[3] == b':' => i + 4,
                5 => i + 3,
                _ => return None,
            };
            sign * (num(i + 1..i + 3)? * 3600 + num(m..m + 2)? * 60)
        }
    };
    Some(secs - off as f64)
}

// 그레고리력 날짜 → 1970-01-01부터의 일수(Howard Hinnant의 days_from_civil).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // 2026-09-20T03:04:05Z. 손 계산: 1970–2025년 56×365+윤년 14 = 20454일,
    // 9월 20일은 그해 262번째 날 → 20716일 × 86400 + 3×3600+4×60+5 = 1789873445.
    const T: f64 = 1_789_873_445.0;

    fn ts(v: Value) -> f64 {
        parse_ts(&v).unwrap_or_else(|| panic!("{v} 못 읽음"))
    }

    #[test]
    fn strings_rfc3339_iso_and_sqlite() {
        assert_eq!(ts(json!("2026-09-20T03:04:05Z")), T);
        assert_eq!(ts(json!("2026-09-20T12:04:05+09:00")), T);
        assert_eq!(ts(json!("2026-09-20T12:04:05+0900")), T);
        assert_eq!(ts(json!("2026-09-19T22:04:05-05:00")), T);
        assert!((ts(json!("2026-09-20T03:04:05.250Z")) - (T + 0.25)).abs() < 1e-6);
        assert!((ts(json!("2026-09-20T12:04:05.123456+09:00")) - (T + 0.123456)).abs() < 1e-6);
        // Z 없으면 UTC
        assert_eq!(ts(json!("2026-09-20T03:04:05")), T);
        assert_eq!(ts(json!("2026-09-20T03:04:05.5")), T + 0.5);
        // SQLite datetime()
        assert_eq!(ts(json!("2026-09-20 03:04:05")), T);
        // 윤일(2000-02-29 = 11016일 × 86400)과 기원
        assert_eq!(ts(json!("2000-02-29T00:00:00Z")), 951_782_400.0);
        assert_eq!(ts(json!("1970-01-01T00:00:00Z")), 0.0);
    }

    #[test]
    fn numbers_by_magnitude() {
        assert_eq!(ts(json!(1_790_000_000u64)), 1_790_000_000.0);
        assert_eq!(ts(json!(1_790_000_000.5)), 1_790_000_000.5);
        assert_eq!(ts(json!(1_790_000_000_123u64)), 1_790_000_000.123);
        assert_eq!(ts(json!(1_790_000_000_123_456u64)), 1_790_000_000.123456);
        // 숫자 문자열도 같은 규칙
        assert_eq!(ts(json!("1790000000")), 1_790_000_000.0);
        assert_eq!(ts(json!("1790000000123")), 1_790_000_000.123);
        assert_eq!(ts(json!("1790000000123456")), 1_790_000_000.123456);
    }

    #[test]
    fn unreadable_is_none() {
        for v in [
            json!(null),
            json!(true),
            json!({}),
            json!([]),
            json!(""),
            json!("yesterday"),
            json!("NaN"),
            json!("inf"),
            json!("2026-13-01T00:00:00Z"),
            json!("2026-09-32T00:00:00Z"),
            json!("2026-09-20T24:00:00Z"),
            json!("2026-09-20T03:04"),
            json!("2026-09-20T03:04:05."),
            json!("2026-09-20T03:04:05+9"),
            json!("2026-09-20T03:04:05 PST"),
            json!("2026-+9-20T03:04:05Z"),
        ] {
            assert_eq!(parse_ts(&v), None, "{v}");
        }
    }

    #[test]
    fn local_date_splits_at_local_midnight() {
        // TZ는 프로세스 전체 값이라 다른 테스트와 겹치지 않게 자식 프로세스에서 본다.
        if std::env::var("TZ").as_deref() != Ok("Asia/Seoul") {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "watch::time::tests::local_date_splits_at_local_midnight",
                    "--exact",
                ])
                .env("TZ", "Asia/Seoul")
                .output()
                .unwrap();
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success() && text.contains("ok. 1 passed"),
                "{text}"
            );
            return;
        }
        // 서울 자정 = 2026-09-20T15:00:00Z = 1789916400
        assert_eq!(local_date(1_789_916_399.0), "2026-09-20");
        assert_eq!(local_date(1_789_916_400.0), "2026-09-21");
        assert_eq!(local_date(ts(json!("2026-09-20T15:00:00Z"))), "2026-09-21");
        assert_eq!(local_date(ts(json!("2026-09-20 14:59:59"))), "2026-09-20");
    }
}
