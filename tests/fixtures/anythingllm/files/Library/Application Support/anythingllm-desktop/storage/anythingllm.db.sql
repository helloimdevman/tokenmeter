-- 스키마와 행: TokenTracker test/anythingllm-parser.test.js:24-39,112-254 (MIT).
CREATE TABLE workspace_chats (id INTEGER PRIMARY KEY AUTOINCREMENT, workspaceId INTEGER NOT NULL,
  prompt TEXT NOT NULL, response TEXT NOT NULL, include BOOLEAN DEFAULT true,
  createdAt DATETIME DEFAULT CURRENT_TIMESTAMP, lastUpdatedAt DATETIME DEFAULT CURRENT_TIMESTAMP);
INSERT INTO workspace_chats (workspaceId, prompt, response, include, createdAt, lastUpdatedAt) VALUES
  (1, 'never read', '{"text":"never read","sources":[{"text":"never read"}],"metrics":{"prompt_tokens":100,"completion_tokens":25,"total_tokens":130,"model":"deepseek-v4","provider":"DeepSeek"}}', 1, 1784037900000, 1784037900000),
  (1, 'never read', 'not-json', 1, '2026-07-14 14:10:00', '2026-07-14 14:10:00'),
  (1, 'never read', '{"text":"never read","metrics":{"prompt_tokens":50,"completion_tokens":10,"total_tokens":60,"model":"deepseek-v4","provider":"DeepSeek"}}', 1, '2026-07-14 14:20:00', '2026-07-14 14:20:00'),
  (1, 'never read', '{}', 0, 1784045100000, 1784045100000);
