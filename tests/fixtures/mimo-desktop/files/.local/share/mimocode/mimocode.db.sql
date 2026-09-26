-- MiMo Code와 MiMo 데스크톱이 같이 쓰는 mimocode.db(OpenCode v1 표). mimo-code·mimo-desktop fixture가 같은 파일을 쓴다.
-- msg_c*/msg_m*: TokenTracker test/mimo-parser.test.js:76-130 (MIT © TokenTracker authors), msg_c1만 데스크톱 세션으로 옮기고 cwd를 바꿈.
-- root_row/fork_*: tokscale sessions/micode.rs:777-830, msg_desktop/msg_cli: :924-1010, msg_sec: :1144-1193의 초 단위 시각
-- (MIT © tokscale authors). tokscale fixture에 없는 time_* 열과 세션 시각을 더함.
CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT, version TEXT, time_created INTEGER);
CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, time_created INTEGER NOT NULL,
                      time_updated INTEGER NOT NULL, data TEXT NOT NULL);
INSERT INTO session VALUES
  ('fork_session', NULL, '/work/mimo', '1.2.3', 1700000000800),
  ('ses_desktop', NULL, '/work/desktop-repo', 'desktop-5198ff5', 1699999999000),
  ('ses_cli', NULL, '/work/mimo', '1.2.3', 1700000004000);
INSERT INTO message VALUES
  -- Claude 기록 복사본(providerID anthropic): 어느 서비스에도 세지 않음
  ('msg_c1', 'ses_desktop', 1780538800000, 1780538800001, '{"id":"msg_c1","sessionID":"ses_desktop","role":"assistant","modelID":"claude-haiku-4-5-20251001","providerID":"anthropic","cost":0,"tokens":{"input":10,"output":300,"reasoning":0,"cache":{"read":800000,"write":5000},"total":805310},"time":{"created":1780538800000,"completed":1780538800100},"path":{"cwd":"/work/mimo","root":"/work/mimo"}}'),
  ('msg_c2', 'ses_proj', 1780538800500, 1780538800501, '{"id":"msg_c2","sessionID":"ses_proj","role":"assistant","modelID":"claude-opus-4-8","providerID":"anthropic","cost":0,"tokens":{"input":1000,"output":5000,"reasoning":0,"cache":{"read":900000,"write":50000},"total":956000},"time":{"created":1780538800500,"completed":1780538800600},"path":{"cwd":"/work/mimo","root":"/work/mimo"}}'),
  ('msg_c3', 'ses_proj', 1780538800700, 1780538800701, '{"id":"msg_c3","sessionID":"ses_proj","role":"assistant","modelID":"mimo-v2.5-pro","providerID":"anthropic","cost":0,"tokens":{"input":200,"output":50,"reasoning":0,"cache":{"read":0,"write":0},"total":250},"time":{"created":1780538800700,"completed":1780538800800},"path":{"cwd":"/work/mimo","root":"/work/mimo"}}'),
  ('msg_m1', 'ses_mimo', 1780538801000, 1780538801001, '{"id":"msg_m1","sessionID":"ses_mimo","role":"assistant","modelID":"mimo-auto","providerID":"mimo","cost":0,"tokens":{"input":500,"output":25,"reasoning":0,"cache":{"read":100,"write":0},"total":625},"time":{"created":1780538801000,"completed":1780538801100},"path":{"cwd":"/work/mimo","root":"/work/mimo"}}'),
  ('msg_m3', 'ses_mimo', 1780538802000, 1780538802001, '{"id":"msg_m3","sessionID":"ses_mimo","role":"assistant","modelID":"mimo-v2.5-pro-ultraspeed","providerID":"xiaomi","cost":0,"tokens":{"input":30,"output":10,"reasoning":0,"cache":{"read":0,"write":0},"total":40},"time":{"created":1780538802000,"completed":1780538802100},"path":{"cwd":"/work/mimo","root":"/work/mimo"}}'),
  -- 포크: fork_session이 생기기 전 시각의 복사본은 세지 않음
  ('root_row', 'root_session', 1700000000000, 1700000000500, '{"role":"assistant","modelID":"mimo-v2.5-pro","providerID":"mimo","cost":0.05,"tokens":{"input":1000,"output":500,"reasoning":25,"cache":{"read":200,"write":50}},"time":{"created":1700000000000.0,"completed":1700000000500.0},"path":{"root":"/work/mimo"}}'),
  ('fork_copy_row', 'fork_session', 1700000000900, 1700000000900, '{"role":"assistant","modelID":"mimo-v2.5-pro","providerID":"mimo","cost":0.05,"tokens":{"input":1000,"output":500,"reasoning":25,"cache":{"read":200,"write":50}},"time":{"created":1700000000000.0,"completed":1700000000500.0},"path":{"root":"/work/mimo"}}'),
  ('fork_new_row', 'fork_session', 1700000001000, 1700000001500, '{"role":"assistant","modelID":"mimo-v2.5-pro","providerID":"mimo","cost":0.08,"tokens":{"input":1300,"output":650,"reasoning":40,"cache":{"read":100,"write":0}},"time":{"created":1700000001000.0,"completed":1700000001500.0},"path":{"root":"/work/mimo"}}'),
  -- 데스크톱 세션(version desktop-…)은 mimo-desktop, 그 밖은 mimo-code
  ('msg_desktop', 'ses_desktop', 1700000000000, 1700000000000, '{"id":"msg_desktop","role":"assistant","modelID":"mimo-x-pro-preview","providerID":"xiaomi","cost":0,"tokens":{"input":1000,"output":200,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1700000000000.0}}'),
  ('msg_cli', 'ses_cli', 1700000005000, 1700000005000, '{"id":"msg_cli","role":"assistant","modelID":"mimo-v2.5-pro","providerID":"mimo","cost":0,"tokens":{"input":500,"output":100,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1700000005000.0}}'),
  -- 초 단위 시각: 세션 시각(밀리초)과 단위를 맞춰 비교해야 포크로 버리지 않는다
  ('msg_sec', 'ses_cli', 1700000006000, 1700000006000, '{"id":"msg_sec","role":"assistant","modelID":"mimo-v2.5-pro","providerID":"mimo","cost":0,"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"time":{"created":1700000006.0,"completed":1700000007.234}}'),
  ('msg_u', 'ses_cli', 1700000005500, 1700000005500, '{"id":"msg_u","role":"user","time":{"created":1700000005500}}');
