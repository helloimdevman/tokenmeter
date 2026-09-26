-- TokenTracker test/goose-parser.test.js:179-198 older schema without accumulated_* (single-turn columns only).
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    model_config_json TEXT,
    provider_name TEXT,
    created_at TEXT NOT NULL,
    total_tokens INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER
);
INSERT INTO sessions VALUES ('old-sess', '{"model_name":"claude-3-haiku"}', 'anthropic', '2026-04-01T12:00:00Z', 500, 400, 100);
