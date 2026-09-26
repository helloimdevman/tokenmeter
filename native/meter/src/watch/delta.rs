//! 레코드 하나에서 나온 토큰 델타.

#[derive(Clone, Debug, Default)]
pub struct TokenDelta {
    pub input_tokens: i64,
    pub cache_read: i64,
    pub cache_write: i64,
    pub output_tokens: i64,
    pub model: String,
    pub service: String,
    pub project: String,
    pub session: String,
    pub vendor: String,
    pub plan: String,
    pub endpoint: String,
    pub cwd: String,
    pub effort: String,
    pub ctx_tokens: i64,
    pub ctx_window: i64,
    pub subagent: bool,
    pub duration_ms: i64,
}

impl TokenDelta {
    pub fn total(&self) -> i64 {
        self.input_tokens + self.cache_read + self.cache_write + self.output_tokens
    }
}

pub(super) type Vector = [i64; 4];
