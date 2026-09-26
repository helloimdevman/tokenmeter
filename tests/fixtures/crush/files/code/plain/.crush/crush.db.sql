-- 스키마: tokscale crates/tokscale-core/src/sessions/crush.rs:252-285 (MIT).
CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT, title TEXT,
  message_count INTEGER NOT NULL DEFAULT 0, prompt_tokens INTEGER NOT NULL DEFAULT 0,
  completion_tokens INTEGER NOT NULL DEFAULT 0, cost REAL NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL DEFAULT 0);
-- 행: crush.rs:449-464(밀리초 시각, 2.0). 다른 프로젝트 DB의 같은 id는 다른 세션이다.
INSERT INTO sessions (id, parent_session_id, title, message_count, cost, updated_at, created_at) VALUES
  ('root-1', NULL, 'Root', 1, 2.0, 1742300000123, 1742300000123);
