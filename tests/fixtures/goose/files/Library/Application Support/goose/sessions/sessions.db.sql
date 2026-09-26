-- TokenTracker test/goose-parser.test.js makeGooseDb schema (no updated_at, no cache columns) and rows
-- sess-1 (:91-126), sess-grow at its first run (:128-176), sess-no-model (:200-219). MIT (c) 2026 xiufengsun.
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    model_config_json TEXT,
    provider_name TEXT,
    created_at TEXT NOT NULL,
    total_tokens INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    accumulated_total_tokens INTEGER,
    accumulated_input_tokens INTEGER,
    accumulated_output_tokens INTEGER
);
INSERT INTO sessions VALUES ('sess-1', '{"model_name":"claude-3-7-sonnet"}', 'anthropic', '2026-05-21T14:00:00Z', 100, 80, 20, 5000, 4000, 800);
INSERT INTO sessions VALUES ('sess-grow', '{"model_name":"gpt-4o"}', 'openai', '2026-05-21T14:00:00Z', NULL, NULL, NULL, 1000, 800, 200);
INSERT INTO sessions VALUES ('sess-no-model', '', 'anthropic', '2026-05-01T00:00:00Z', NULL, NULL, NULL, 1234, NULL, NULL);
