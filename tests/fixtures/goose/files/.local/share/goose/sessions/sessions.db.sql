-- Goose schema v16 sessions table (block/goose crates/goose/src/session/session_manager.rs:1025-1060, the columns we read).
-- session-a: ccusage rust/adapters/goose/src/loader.rs:177-210 (MIT (c) 2025 ryoppippi).
-- 20260920_3 and its child: synthetic, v14+ cache columns and accumulated_cost (research goose.md section 5 sample shape).
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    name TEXT,
    session_type TEXT,
    working_dir TEXT NOT NULL DEFAULT '',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    total_tokens INTEGER,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER,
    accumulated_total_tokens INTEGER,
    accumulated_input_tokens INTEGER,
    accumulated_output_tokens INTEGER,
    accumulated_cache_read_tokens INTEGER,
    accumulated_cache_write_tokens INTEGER,
    accumulated_cost REAL,
    provider_name TEXT,
    model_config_json TEXT,
    parent_session_id TEXT
);
INSERT INTO sessions (id, working_dir, created_at, updated_at, total_tokens, input_tokens, output_tokens,
  accumulated_total_tokens, accumulated_input_tokens, accumulated_output_tokens, provider_name, model_config_json)
  VALUES ('session-a', '/work/a', '2026-05-01 01:02:03', '2026-05-01 01:02:03', 180, 100, 50, 180, 100, 50,
          'anthropic', '{"model_name":"claude-sonnet-4-20250514"}');
INSERT INTO sessions VALUES ('20260920_3', 'title', 'user', '/work/p1', '2026-09-20 07:12:44', '2026-09-20 07:41:02',
  48210, 47100, 1110, 45000, 1900, 610000, 590000, 20000, 540000, 31000, 0.83,
  'anthropic', '{"model_name":"claude-sonnet-4-6","context_limit":200000}', NULL);
INSERT INTO sessions VALUES ('20260920_4', 'sub', 'sub_agent', '/work/p1', '2026-09-20 07:20:00', '2026-09-20 07:25:00',
  1100, 1000, 100, 0, 0, 1100, 1000, 100, 0, 0, NULL,
  'anthropic', '{"model_name":"claude-sonnet-4-6","context_limit":200000}', '20260920_3');
