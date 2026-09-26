-- synthetic OpenCode 1.2+ DB (layout D: message + session_message + session). Rows marked ccusage are
-- ccusage rust/adapters/opencode/src/loader.rs:1377-1475 fixtures (MIT © 2025 ryoppippi) with a role added.
CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, version TEXT, title TEXT,
                      time_created INTEGER, time_updated INTEGER);
CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT, type TEXT, seq INTEGER NOT NULL DEFAULT 0,
                              time_created INTEGER, time_updated INTEGER, data TEXT);
INSERT INTO session VALUES
  ('ses_a', NULL, '/work/app', '1.18.20', 't', 1767311000000, 1767312300000),
  ('ses_b', 'ses_a', '/work/app', '1.18.20', 't', 1767311500000, 1767312200000),
  ('ses_f', NULL, '/work/app', '1.18.20', 't', 1767313000000, 1767313100000),
  ('v2-session', NULL, '/work/v2', '2.0.0', 't', 1767311900000, 1767312000001);
INSERT INTO message VALUES
  -- ccusage 1377-1406 + reasoning 30: in 120, cache 12/24, out 60 + 30
  ('db-msg-1', 'ses_a', 1767312000000, 1767312004000,
   '{"role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000,"completed":1767312004000},"tokens":{"input":120,"output":60,"reasoning":30,"cache":{"read":12,"write":24}},"cost":0.03}'),
  ('db-msg-2', 'ses_a', 1767312100000, 1767312100000, '{"role":"user","time":{"created":1767312100000}}'),
  -- 하위 세션(ses_b의 부모 ses_a)
  ('db-msg-3', 'ses_b', 1767312200000, 1767312201000,
   '{"role":"assistant","providerID":"openai","modelID":"gpt-5","time":{"created":1767312200000,"completed":1767312201000},"tokens":{"input":200,"output":20,"reasoning":0,"cache":{"read":0,"write":0}},"cost":0}'),
  -- 포크 복사본: ses_f가 생기기 전 시각 → 세지 않음
  ('db-msg-4', 'ses_f', 1767313000000, 1767313000000,
   '{"role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000,"completed":1767312004000},"tokens":{"input":120,"output":60,"reasoning":30,"cache":{"read":12,"write":24}},"cost":0.03}'),
  ('db-msg-5', 'ses_f', 1767313100000, 1767313101000,
   '{"role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767313100000,"completed":1767313101000},"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"cost":0.01}'),
  -- 스트림 시작 자리표시(토큰 0). step-2가 최종값으로 고쳐 쓴다
  ('db-msg-6', 'ses_a', 1767312300000, 1767312300000,
   '{"role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312300000},"tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"cost":0}');
INSERT INTO session_message VALUES
  ('msg-v2-user', 'v2-session', 'user', 1, 1767312000000, 1767312000000, '{"text":"t","time":{"created":1767312000000}}'),
  -- ccusage 1408-1475 그대로
  ('msg-v2-assistant', 'v2-session', 'assistant', 2, 1767312000001, 1767312000001,
   '{"model":{"id":"gpt-test","providerID":"openai"},"time":{"created":1767312000000},"tokens":{"input":120,"output":60,"reasoning":10,"cache":{"read":12,"write":24}},"cost":0.03}'),
  -- v1 행과 같은 id(전이 배치 D): 한 번만
  ('db-msg-3', 'ses_b', 'assistant', 1, 1767312200000, 1767312201000,
   '{"model":{"id":"gpt-5","providerID":"openai"},"time":{"created":1767312200000,"completed":1767312201000},"tokens":{"input":200,"output":20,"reasoning":0,"cache":{"read":0,"write":0}},"cost":0}');
