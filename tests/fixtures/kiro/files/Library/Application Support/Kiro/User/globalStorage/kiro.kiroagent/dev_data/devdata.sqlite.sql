-- TokenTracker test/kiro-dual-install.test.js:42-58, :75-90 native install (MIT (c) 2026 xiufengsun).
CREATE TABLE tokens_generated (id INTEGER PRIMARY KEY, model TEXT, provider TEXT,
  tokens_prompt INTEGER, tokens_generated INTEGER, timestamp TEXT);
INSERT INTO tokens_generated VALUES (1, 'agent', 'kiro', 100, 10, '2026-01-09 10:05:00');
INSERT INTO tokens_generated VALUES (2, 'agent', 'kiro', 200, 20, '2026-01-09 10:10:00');
