-- 스키마: tokscale copilot_session_store.rs:175-208 (MIT). 행 1: 같은 파일 :298-333의 단언값.
-- 행 2·3: TokenTracker test/copilot-session-store-parser.test.js:243-258, :286-322 (MIT).
CREATE TABLE sessions (id TEXT PRIMARY KEY, cwd TEXT, repository TEXT, host_type TEXT,
  branch TEXT, summary TEXT, created_at TEXT, updated_at TEXT);
CREATE TABLE assistant_usage_events (
  id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, turn_index INTEGER,
  model TEXT, copilot_usage_model TEXT,
  input_tokens INTEGER, output_tokens INTEGER, cache_read_tokens INTEGER,
  cache_write_tokens INTEGER, reasoning_tokens INTEGER,
  total_nano_aiu INTEGER, duration_ms INTEGER, created_at TEXT);
INSERT INTO sessions (id, cwd, summary, created_at) VALUES
  ('session-1', '/work/copilot', 'summary text', '2026-07-01 12:00:00'),
  ('session-2', '/work/copilot', NULL, '2026-07-10T09:00:00Z'),
  ('session-3', '/work/copilot', NULL, '2026-07-10T11:00:00Z');
INSERT INTO assistant_usage_events (id, session_id, turn_index, model, copilot_usage_model,
  input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, reasoning_tokens,
  total_nano_aiu, duration_ms, created_at) VALUES
  (1, 'session-1', 0, 'gpt-5.4-mini', NULL, 21343, 100, 0, 20974, 0, 556570000, 1234, '2026-07-01 12:34:56'),
  (2, 'session-2', 0, 'gpt-5.6-luna', NULL, 100, 10, 30, 20, 2, 0, NULL, '2026-07-10T10:00:00Z'),
  (3, 'session-2', 1, 'gpt-5.6-luna', NULL, 125, 7, 20, 0, 3, 0, NULL, '2026-07-10T10:30:05Z'),
  (4, 'session-3', 0, 'auto', 'claude-sonnet-4.6', 40, 5, 0, 0, 0, 0, NULL, NULL),
  (5, 'session-3', 1, 'auto', NULL, 0, 0, 0, 0, 0, 0, NULL, '2026-07-10T11:05:00Z'),
  (6, '', 0, 'gpt-5.4-mini', NULL, 999, 9, 0, 0, 0, 0, NULL, '2026-07-10T11:06:00Z');
