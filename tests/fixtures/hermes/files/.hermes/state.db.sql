-- Hermes before session_model_usage (schema < v20): only the sessions totals.
-- Rows: ccusage rust/adapters/hermes/src/loader.rs:138-198 (session-1) and
-- tokscale crates/tokscale-core/tests/hermes.rs legacy_skips_empty_sessions (legacy-*).
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    model TEXT,
    started_at REAL NOT NULL,
    message_count INTEGER DEFAULT 0,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    reasoning_tokens INTEGER DEFAULT 0,
    billing_provider TEXT,
    estimated_cost_usd REAL,
    actual_cost_usd REAL
);
INSERT INTO sessions VALUES ('session-1', 'cli', 'claude-sonnet-4-20250514', 1750000000.25, 42, 1200, 300, 50, 20, 10, 'anthropic', 0.12, 0.34);
INSERT INTO sessions (id, source, model, started_at, message_count, input_tokens, output_tokens, reasoning_tokens, billing_provider, estimated_cost_usd, actual_cost_usd)
  VALUES ('legacy-valid', 'telegram', 'gpt-5.4', 1775001102.0, 3, 100, 20, 5, NULL, 1.25, NULL);
INSERT INTO sessions (id, source, model, started_at, message_count) VALUES ('legacy-empty', 'telegram', 'gpt-5.4', 1775001103.0, 9);
INSERT INTO sessions (id, source, model, started_at, message_count, input_tokens) VALUES ('legacy-no-model', 'cli', NULL, 1775001104.0, 1, 500);
