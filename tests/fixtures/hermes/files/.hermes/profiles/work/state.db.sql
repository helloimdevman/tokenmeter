-- Hermes v20+ profile DB: per-route cumulative rows in session_model_usage.
-- Schema and rows: tokscale crates/tokscale-core/tests/hermes.rs create_test_db and
-- :130-201 (session-1), :350-520 (session-multi), :656-763 (backfilled / not-backfilled),
-- :765-821 (partial), the provider split test (session-split), :979-1059 (session-cost),
-- the cost-only test (session-cost-only). MIT (c) 2025 Junho Yeo.
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
CREATE TABLE session_model_usage (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    billing_provider TEXT NOT NULL DEFAULT '',
    billing_base_url TEXT NOT NULL DEFAULT '',
    billing_mode TEXT NOT NULL DEFAULT '',
    task TEXT NOT NULL DEFAULT '',
    api_call_count INTEGER NOT NULL DEFAULT 0,
    input_tokens INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
    estimated_cost_usd REAL NOT NULL DEFAULT 0,
    actual_cost_usd REAL NOT NULL DEFAULT 0,
    cost_status TEXT,
    cost_source TEXT,
    first_seen REAL,
    last_seen REAL,
    PRIMARY KEY (session_id, model, billing_provider, billing_base_url, billing_mode, task)
);
INSERT INTO sessions (id, source, model, started_at, message_count) VALUES ('session-1', 'cli', 'claude-sonnet-4', 1750000000.25, 42);
INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd)
  VALUES ('session-1', 'claude-sonnet-4', 'anthropic', '', '', '', 1200, 300, 50, 20, 10, 0.12, 0.34);

INSERT INTO sessions (id, source, model, started_at, message_count) VALUES ('session-multi', 'desktop', 'glm-5.2', 1775002000.0, 15);
INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd) VALUES
  ('session-multi', 'glm-5.2', 'kilocode', '', '', '', 5000, 800, 100, 0, 200, 0.0, 0.0),
  ('session-multi', 'mimo-v2.5-free', 'opencode-zen', '', '', '', 3000, 500, 0, 0, 0, 0.0, 0.0),
  ('session-multi', 'cohere/north-mini-code:free', 'bifrost', '', '', '', 900, 100, 0, 0, 0, 0.0, 0.0),
  ('session-multi', 'cohere/north-mini-code:free', 'bifrost', '', '', 'title_generation', 100, 50, 0, 0, 0, 0.0, 0.0);

INSERT INTO sessions (id, source, model, started_at, message_count, input_tokens, output_tokens, billing_provider, actual_cost_usd)
  VALUES ('session-backfilled', 'cli', 'claude-sonnet-4', 1775003000.0, 5, 1200, 300, 'anthropic', 0.10);
INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd)
  VALUES ('session-backfilled', 'claude-sonnet-4', 'anthropic', '', '', '', 1200, 300, 0, 0, 0, 0.0, 0.10);
INSERT INTO sessions (id, source, model, started_at, message_count, input_tokens, output_tokens, billing_provider, actual_cost_usd)
  VALUES ('session-not-backfilled', 'desktop', 'claude-opus-4-6', 1775003001.0, 42, 999000, 12000, 'anthropic', 2.50);

INSERT INTO sessions (id, source, model, started_at, message_count, input_tokens, output_tokens)
  VALUES ('session-partial', 'cli', 'glm-5.2', 1775003100.0, 9, 5000, 900);
INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd)
  VALUES ('session-partial', 'glm-5.2', 'kilocode', '', '', '', 3000, 400, 0, 0, 0, 0.0, 0.0);

INSERT INTO sessions (id, source, model, started_at, message_count) VALUES ('session-split', 'desktop', 'glm-5.2', 1775004000.0, 7);
INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd) VALUES
  ('session-split', 'glm-5.2', 'kilocode', '', '', '', 1000, 0, 0, 0, 0, 0.0, 0.0),
  ('session-split', 'glm-5.2', 'opencode-zen', '', '', '', 2000, 0, 0, 0, 0, 0.0, 0.0);

INSERT INTO sessions (id, source, model, started_at, message_count) VALUES ('session-cost', 'cli', 'gpt-5.4', 1775005000.0, 4);
INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd) VALUES
  ('session-cost', 'gpt-5.4', 'openai', '', '', '', 1000, 200, 0, 0, 0, 0.0, 0.50),
  ('session-cost', 'gpt-5.4', 'openai', '', '', 'title_generation', 100, 20, 0, 0, 0, 0.05, 0.0);

INSERT INTO sessions (id, source, model, started_at, message_count) VALUES ('session-cost-only', 'cli', 'gpt-5.4', 1775006000.0, 1);
INSERT INTO session_model_usage (session_id, model, billing_provider, billing_base_url, billing_mode, task, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens, estimated_cost_usd, actual_cost_usd)
  VALUES ('session-cost-only', 'gpt-5.4', 'openai', '', '', '', 0, 0, 0, 0, 0, 0.07, 0.0);
