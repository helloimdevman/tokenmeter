-- TokenTracker test/kiro-dual-install.test.js:42-58, :80-84 second install; ids restart at 1 (MIT (c) 2026 xiufengsun).
CREATE TABLE tokens_generated (id INTEGER PRIMARY KEY, model TEXT, provider TEXT,
  tokens_prompt INTEGER, tokens_generated INTEGER, timestamp TEXT);
INSERT INTO tokens_generated VALUES (1, 'agent', 'kiro', 1000, 100, '2026-01-09 11:05:00');
INSERT INTO tokens_generated VALUES (2, 'agent', 'kiro', 2000, 200, '2026-01-09 11:10:00');
INSERT INTO tokens_generated VALUES (3, 'agent', 'kiro', 3000, 300, '2026-01-09 11:20:00');
