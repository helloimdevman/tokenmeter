-- 스키마: tokscale crates/tokscale-core/src/sessions/crush.rs:252-285 (MIT).
CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT, title TEXT,
  message_count INTEGER NOT NULL DEFAULT 0, prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0, cost REAL NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL DEFAULT 0);
-- 행: crush.rs:386-424(root-1 30.0과 자식 99.0), :426-447(root-2 4.5). 루트 비용에 자식이 이미 들어 있다.
INSERT INTO sessions (id, parent_session_id, title, message_count, prompt_tokens, completion_tokens, cost, updated_at, created_at) VALUES
  ('root-1', NULL, 'Root', 5, 38400, 820, 30.0, 1742386400, 1742300000),
  ('child-1', 'root-1', 'Child', 2, 12000, 300, 99.0, 1742342001, 1742300100),
  ('root-2', NULL, 'Root', 3, 900, 40, 4.5, 1742342000, 1742300000),
  ('root-zero', NULL, 'Root', 0, 0, 0, 0, 1742342000, 1742300000);
