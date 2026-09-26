-- Kilo CLI kilo.db: OpenCode v1 message 표. session 표는 upstream fixture에 없다(조인 없는 쿼리로 떨어짐).
-- msg_kilo_001/002: TokenTracker test/kilo-parser.test.js:41-90 (MIT © TokenTracker authors), cwd만 바꿈.
-- row-1(msg-1): ccusage rust/adapters/kilo/src/loader.rs:129-162 (MIT © 2025 ryoppippi), time_* 열을 더함.
CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, time_updated INTEGER, data TEXT);
INSERT INTO message VALUES
  ('msg_kilo_001', 'sess_kilo_a', 1778568000000, 1778568000001,
   '{"id":"msg_kilo_001","sessionID":"sess_kilo_a","role":"assistant","modelID":"inclusionai/ring-2.6-1t:free","providerID":"kilo","tokens":{"input":1000,"output":50,"reasoning":10,"cache":{"read":200,"write":0},"total":1260},"time":{"created":1778568000000,"completed":1778568000100},"path":{"cwd":"/work/kilo","root":"/work/kilo"}}'),
  ('msg_kilo_002', 'sess_kilo_a', 1778568001000, 1778568001001,
   '{"id":"msg_kilo_002","sessionID":"sess_kilo_a","role":"assistant","modelID":"kilo-auto/free","providerID":"kilo","tokens":{"input":500,"output":25,"reasoning":0,"cache":{"read":100,"write":0},"total":625},"time":{"created":1778568001000,"completed":1778568001100},"path":{"cwd":"/work/kilo","root":"/work/kilo"}}'),
  ('row-1', 'session-a', 1767312000000, 1767312000000,
   '{"id":"msg-1","role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312000000},"tokens":{"input":100,"output":50,"reasoning":5,"cache":{"read":10,"write":20}},"cost":0.02,"agent":"build","path":{"cwd":"/work/kilo"}}'),
  ('row-2', 'session-a', 1767312001000, 1767312001000, '{"id":"msg-2","role":"user","time":{"created":1767312001000}}');
