//! JSON/JSONL 로그에서 토큰 델타를 뽑는다. Python ServiceReader 의 핵심 규약만 옮긴다.

use std::time::{SystemTime, UNIX_EPOCH};

pub mod antigravity;
pub mod checkpoint;
pub mod cond;
pub mod decode;
pub mod delta;
pub mod expr;
pub mod ledger;
pub mod probe;
pub mod reader;
pub mod record;
pub mod roots;
pub mod spec;
pub mod sqlite;
pub mod time;
#[cfg(test)]
mod tests;

pub use delta::*;
pub use expr::dig;
pub use reader::*;
pub(crate) use roots::expand_home;
pub use spec::*;

pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}
