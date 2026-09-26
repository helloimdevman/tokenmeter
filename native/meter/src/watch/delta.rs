//! 레코드 하나에서 나온 토큰 델타.

#[derive(Clone, Debug)]
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
    /// 레코드 시각(유닉스 초). 0이면 지금.
    pub at: f64,
    /// 이 델타가 세는 호출 수(F2). 키 장부가 정하기 전(2.2)까지 기본 1.
    pub calls: u32,
    /// 로그에 적힌 비용. 0보다 크고 유한하면 가격표 대신 쓴다(F9).
    pub cost_usd: Option<f64>,
    /// 경로 셀의 시간 칸(`api_out`·`api_ms`)에만 더하는 속도 표본(스펙 10절).
    pub speed_only: bool,
}

impl Default for TokenDelta {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            output_tokens: 0,
            model: String::new(),
            service: String::new(),
            project: String::new(),
            session: String::new(),
            vendor: String::new(),
            plan: String::new(),
            endpoint: String::new(),
            cwd: String::new(),
            effort: String::new(),
            ctx_tokens: 0,
            ctx_window: 0,
            subagent: false,
            duration_ms: 0,
            at: 0.0,
            calls: 1,
            cost_usd: None,
            speed_only: false,
        }
    }
}

impl TokenDelta {
    pub fn total(&self) -> i64 {
        self.input_tokens + self.cache_read + self.cache_write + self.output_tokens
    }
}
