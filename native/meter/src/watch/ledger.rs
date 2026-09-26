//! 키 장부와 기준값(F2). 같은 키의 레코드는 칸마다 최댓값만 남기고(delta),
//! 누적 스트림은 기준값과의 차이를 낸다(cumulative). 키 원문은 두지 않고 FNV-1a 64비트 해시만 둔다.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// input, cache_read, cache_write, output, cost_usd, duration_ms
pub type Vals = [f64; 6];

/// 커밋 때 버리는 나이: 밀린 기록 창 7일 + 여유 10분(스펙 4.4).
const KEEP_SECS: f64 = 7.0 * 86_400.0 + 600.0;

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3)
    })
}

pub fn key_hash(key: &str) -> u64 {
    fnv1a64(key.as_bytes())
}

/// 키 없는 delta 소스의 키: 키를 정렬한 압축 JSON의 해시(스펙 F2).
/// ponytail: serde_json 객체가 BTreeMap(`preserve_order` 꺼짐)이라 `to_string`이 이미 정렬 순이다.
/// 그 기능이 켜지면 `record_hash_ignores_key_order`가 깨지니 그때 정렬해 쓰는 함수를 둔다.
pub fn record_hash(rec: &Value) -> u64 {
    fnv1a64(rec.to_string().as_bytes())
}

/// 호출로 치는 칸: 토큰 넷과 비용. 시간만 늘어난 것은 호출이 아니다(델타 버리기 규칙과 같다).
fn counts(v: &Vals) -> bool {
    v[..5].iter().any(|x| *x > 0.0)
}

struct Entry {
    seen: f64,
    vals: Vals,
    /// 최근 사용 순. 같은 시계 값 안에서 순서를 가른다.
    tick: u64,
}

pub struct Ledger {
    clock: fn() -> f64,
    cap: usize,
    tick: u64,
    keys: HashMap<u64, Entry>,
    dirty: HashSet<u64>,
}

impl Ledger {
    pub fn new(clock: fn() -> f64, cap: usize) -> Self {
        Self {
            clock,
            cap,
            tick: 0,
            keys: HashMap::new(),
            dirty: HashSet::new(),
        }
    }

    /// delta + 키: 칸마다 지금까지의 최댓값보다 커진 만큼. calls = 이 키가 처음으로 0이 아닌 값을 낸 때만 true.
    pub fn grow(&mut self, key: u64, v: Vals) -> (Vals, bool) {
        self.tick += 1;
        let seen = (self.clock)();
        let e = self.keys.entry(key).or_insert_with(|| {
            self.dirty.insert(key);
            Entry {
                seen,
                vals: [0.0; 6],
                tick: 0,
            }
        });
        e.seen = seen;
        e.tick = self.tick;
        let first = !counts(&e.vals);
        let mut g = [0.0; 6];
        for i in 0..6 {
            if v[i] > e.vals[i] {
                g[i] = v[i] - e.vals[i];
                e.vals[i] = v[i];
            }
        }
        if g.iter().any(|x| *x > 0.0) {
            self.dirty.insert(key);
        }
        (g, first && counts(&g))
    }

    pub fn has(&self, key: u64) -> bool {
        self.keys.contains_key(&key)
    }

    pub fn get(&self, key: u64) -> Option<Vals> {
        self.keys.get(&key).map(|e| e.vals)
    }

    /// 커밋 때만 부른다: 본 시각이 7일 10분보다 오래된 키를 버린다. 넘치면 오래 안 본 키부터.
    pub fn prune(&mut self) {
        let cutoff = (self.clock)() - KEEP_SECS;
        self.keys.retain(|_, e| e.seen >= cutoff);
        if self.keys.len() > self.cap {
            // ponytail: 넘칠 때만 전체 정렬(O(n log n)). 커밋마다 넘치면 순서 있는 색인을 둔다.
            let mut order: Vec<(f64, u64, u64)> = self
                .keys
                .iter()
                .map(|(k, e)| (e.seen, e.tick, *k))
                .collect();
            order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
            for (_, _, k) in &order[..self.keys.len() - self.cap] {
                self.keys.remove(k);
            }
        }
    }

    /// 마지막 커밋 뒤 새로 생기거나 늘어난 키(.keys 덧붙이기용): (hash, [본 시각, Vals…])
    pub fn take_dirty(&mut self) -> Vec<(u64, [f64; 7])> {
        let dirty = std::mem::take(&mut self.dirty);
        dirty
            .into_iter()
            .filter_map(|k| self.keys.get(&k).map(|e| (k, row(e))))
            .collect()
    }

    /// `.keys`에서 읽은 줄. 같은 키는 뒤 것이 이긴다.
    pub fn load(&mut self, entries: impl IntoIterator<Item = (u64, [f64; 7])>) {
        for (k, r) in entries {
            self.tick += 1;
            let mut vals = [0.0; 6];
            vals.copy_from_slice(&r[1..]);
            self.keys.insert(
                k,
                Entry {
                    seen: r[0],
                    vals,
                    tick: self.tick,
                },
            );
        }
    }

    pub fn live(&self) -> Vec<(u64, [f64; 7])> {
        self.keys.iter().map(|(k, e)| (*k, row(e))).collect()
    }
}

fn row(e: &Entry) -> [f64; 7] {
    let mut r = [e.seen; 7];
    r[1..].copy_from_slice(&e.vals);
    r
}

/// cumulative: 기준값과의 차이. 한 칸이라도 줄면 새 기준을 잡고 0.
/// `base`가 없으면 `other`(같은 키의 다른 파일 항목 기준값)를 본다. 둘 다 없으면 None(문턱이 정함, 4.4).
/// 어느 경우든 `base`는 `v`가 된다. calls는 0 기준에서 처음 0이 아닌 값을 낼 때만 true.
pub fn diff(base: &mut Option<Vals>, other: Option<Vals>, v: Vals) -> Option<(Vals, bool)> {
    let prev = base.or(other);
    *base = Some(v);
    let prev = prev?;
    if (0..6).any(|i| v[i] < prev[i]) {
        return Some(([0.0; 6], false));
    }
    let mut d = [0.0; 6];
    for i in 0..6 {
        d[i] = v[i] - prev[i];
    }
    Some((d, !counts(&prev) && counts(&d)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    thread_local! {
        static NOW: Cell<f64> = const { Cell::new(1_790_000_000.0) };
    }
    fn clock() -> f64 {
        NOW.with(|c| c.get())
    }
    fn set_now(t: f64) {
        NOW.with(|c| c.set(t));
    }
    fn out(n: f64) -> Vals {
        [0.0, 0.0, 0.0, n, 0.0, 0.0]
    }

    const DAY: f64 = 86_400.0;
    const MIN: f64 = 60.0;

    #[test]
    fn fnv1a64_known_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(key_hash("a"), fnv1a64(b"a"));
    }

    #[test]
    fn max_keep_emits_growth_per_slot() {
        let mut l = Ledger::new(clock, 100);
        let k = key_hash("m1|r1");
        assert_eq!(l.grow(k, out(31.0)), (out(31.0), true));
        assert_eq!(l.grow(k, out(31.0)), (out(0.0), false));
        assert_eq!(l.grow(k, out(300.0)), (out(269.0), false));
        // 칸마다 따로: input만 늘고 output은 줄어든 복사본
        let (g, c) = l.grow(k, [5.0, 0.0, 0.0, 200.0, 0.0, 0.0]);
        assert_eq!((g, c), ([5.0, 0.0, 0.0, 0.0, 0.0, 0.0], false));
        assert_eq!(l.get(k), Some([5.0, 0.0, 0.0, 300.0, 0.0, 0.0]));
    }

    #[test]
    fn zero_first_copy_does_not_burn_key() {
        let mut l = Ledger::new(clock, 100);
        let k = key_hash("k");
        assert_eq!(l.grow(k, out(0.0)), (out(0.0), false));
        assert!(l.has(k), "0도 적어 둔다");
        assert_eq!(l.grow(k, out(50.0)), (out(50.0), true));
        assert_eq!(l.grow(k, out(50.0)), (out(0.0), false));
    }

    #[test]
    fn cost_and_duration_follow_the_same_rule() {
        let mut l = Ledger::new(clock, 100);
        let k = key_hash("c");
        let v = |cost: f64, ms: f64| [0.0, 0.0, 0.0, 0.0, cost, ms];
        // 토큰 0이어도 비용이 처음 나면 호출이다(crush)
        assert_eq!(l.grow(k, v(0.25, 100.0)), (v(0.25, 100.0), true));
        assert_eq!(l.grow(k, v(0.25, 100.0)), (v(0.0, 0.0), false));
        assert_eq!(l.grow(k, v(0.75, 250.0)), (v(0.5, 150.0), false));
        // 시간만 늘어난 것은 호출이 아니다
        let d = key_hash("d");
        assert_eq!(l.grow(d, v(0.0, 80.0)), (v(0.0, 80.0), false));
        assert_eq!(l.grow(d, out(9.0)), (out(9.0), true));
    }

    #[test]
    fn cumulative_diffs_and_rebaselines_on_any_drop() {
        let mut base = None;
        assert_eq!(
            diff(&mut base, None, out(10.0)),
            None,
            "처음 본 스트림은 문턱이 정한다"
        );
        assert_eq!(base, Some(out(10.0)));
        assert_eq!(diff(&mut base, None, out(25.0)), Some((out(15.0), false)));
        let dropped = [1.0, 0.0, 0.0, 30.0, 0.0, 0.0];
        let mut up = out(25.0);
        up[0] = 3.0;
        assert_eq!(
            diff(&mut base, None, up),
            Some(([3.0, 0.0, 0.0, 0.0, 0.0, 0.0], false))
        );
        assert_eq!(
            diff(&mut base, None, dropped),
            Some(([0.0; 6], false)),
            "한 칸이라도 줄면 0"
        );
        assert_eq!(base, Some(dropped));
        assert_eq!(
            diff(&mut base, None, [1.0, 0.0, 0.0, 34.0, 0.0, 0.0]),
            Some((out(4.0), false))
        );
        // 0 기준에서 처음 나는 값이 호출이다
        let mut zero = Some([0.0; 6]);
        assert_eq!(diff(&mut zero, None, out(7.0)), Some((out(7.0), true)));
        // 비용·시간도 누적치로 본다
        let mut cost = Some([0.0, 0.0, 0.0, 0.0, 0.25, 1000.0]);
        assert_eq!(
            diff(&mut cost, None, [0.0, 0.0, 0.0, 0.0, 0.75, 1500.0]),
            Some(([0.0, 0.0, 0.0, 0.0, 0.5, 500.0], false))
        );
    }

    #[test]
    fn cumulative_uses_same_key_from_other_file() {
        let mut base = None;
        assert_eq!(
            diff(&mut base, Some(out(40.0)), out(45.0)),
            Some((out(5.0), false))
        );
        assert_eq!(base, Some(out(45.0)), "기준값은 이 파일 것으로 잡는다");
        // 이 파일의 기준값이 있으면 다른 파일 것은 보지 않는다
        assert_eq!(
            diff(&mut base, Some(out(0.0)), out(50.0)),
            Some((out(5.0), false))
        );
    }

    #[test]
    fn record_hash_ignores_key_order() {
        let a = json!({"a": 1, "b": {"x": [1, 2], "y": "z"}});
        let b = json!({"b": {"y": "z", "x": [1, 2]}, "a": 1});
        assert_eq!(record_hash(&a), record_hash(&b));
        assert_eq!(
            record_hash(&a),
            fnv1a64(br#"{"a":1,"b":{"x":[1,2],"y":"z"}}"#)
        );
        assert_ne!(
            record_hash(&a),
            record_hash(&json!({"a": 2, "b": {"x": [1, 2], "y": "z"}}))
        );
    }

    #[test]
    fn keys_expire_7d10m_after_last_seen() {
        let t = 1_790_000_000.0;
        set_now(t);
        let mut l = Ledger::new(clock, 100);
        let (a, b) = (key_hash("a"), key_hash("b"));
        l.grow(a, out(1.0));
        l.grow(b, out(1.0));
        set_now(t + 2.0 * DAY);
        l.grow(b, out(1.0)); // 다시 읽으면 본 시각이 새로 잡힌다
        set_now(t + 7.0 * DAY + 9.0 * MIN);
        l.prune();
        assert!(l.has(a) && l.has(b));
        set_now(t + 7.0 * DAY + 11.0 * MIN);
        l.prune();
        assert!(!l.has(a), "7일 11분 전에 본 키는 버린다");
        assert!(l.has(b));
    }

    #[test]
    fn cap_evicts_least_recently_seen() {
        set_now(1_790_000_000.0);
        let mut l = Ledger::new(clock, 10);
        for i in 0..20u64 {
            l.grow(i, out(1.0));
        }
        l.grow(0, out(1.0)); // 같은 시계에서도 최근에 본 순서가 이긴다
        l.prune();
        assert_eq!(l.live().len(), 10);
        assert!(l.has(0));
        assert!((1..=10).all(|i| !l.has(i)), "오래 안 본 키부터 버린다");
        assert!((11..20).all(|i| l.has(i)));
    }

    #[test]
    fn dirty_keys_drain_once() {
        set_now(1_790_000_000.0);
        let mut l = Ledger::new(clock, 100);
        l.load([(7, [1_789_000_000.0, 0.0, 0.0, 0.0, 5.0, 0.0, 0.0])]);
        assert!(l.take_dirty().is_empty(), "불러온 키는 이미 디스크에 있다");
        l.grow(1, out(0.0));
        l.grow(7, out(5.0)); // 늘지 않음
        assert_eq!(
            l.take_dirty(),
            vec![(1, [1_790_000_000.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0])]
        );
        assert!(l.take_dirty().is_empty());
        l.grow(7, out(8.0));
        assert_eq!(
            l.take_dirty(),
            vec![(7, [1_790_000_000.0, 0.0, 0.0, 0.0, 8.0, 0.0, 0.0])]
        );
        assert_eq!(l.live().len(), 2);
    }
}
