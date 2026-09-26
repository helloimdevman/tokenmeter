-- 스키마: TokenTracker test/unsloth-parser.test.js:49-76 (MIT). 행: tokscale sessions/unsloth.rs:356-533 (MIT)
-- message-1·request-1(:356-424), local-message(:455-504), fallback(:426-453), 건너뛰는 행(:506-533),
-- request-2: TokenTracker :137-143(오류로 끝난 요청도 센다).
CREATE TABLE chat_threads (id TEXT PRIMARY KEY, model_id TEXT);
CREATE TABLE chat_messages (id TEXT PRIMARY KEY, thread_id TEXT, role TEXT, content_json TEXT,
  attachments_json TEXT, metadata_json TEXT, created_at INTEGER);
CREATE TABLE api_usage_events (id TEXT PRIMARY KEY, subject TEXT, endpoint TEXT, model TEXT,
  status TEXT, prompt_tokens INTEGER, completion_tokens INTEGER, total_tokens INTEGER, created_at INTEGER);
INSERT INTO chat_threads VALUES ('thread-1', 'thread-fallback');
INSERT INTO chat_messages VALUES
  ('message-1', 'thread-1', 'assistant', '{"text":"never read"}', '[]',
   '{"privatePreview":"never select this","contextUsage":{"promptTokens":100,"completionTokens":40,"totalTokens":140,"cachedTokens":30,"cacheWriteTokens":10,"reasoningTokens":5,"modelId":"requested-model"},"responseDetails":{"responseModelId":"claude-sonnet-4-6","providerType":"anthropic"}}',
   1788000000123),
  ('local-message', 'thread-1', 'assistant', '{}', '[]',
   '{"contextUsage":{"promptTokens":10,"completionTokens":2,"totalTokens":12},"responseDetails":{"responseModelId":"local-model","providerType":"local"}}',
   1788000000),
  ('fallback-message', 'thread-1', 'assistant', '{}', '[]',
   '{"contextUsage":{"promptTokens":5,"completionTokens":3,"totalTokens":8}}', 1788000000),
  ('user-1', 'thread-1', 'user', '{"text":"never read"}', '[]',
   '{"contextUsage":{"promptTokens":10,"totalTokens":10}}', 1788000000),
  ('assistant-bad', 'thread-1', 'assistant', '{}', '[]', 'not-json', 1788000000),
  ('assistant-zero', 'thread-1', 'assistant', '{}', '[]',
   '{"contextUsage":{"promptTokens":0,"completionTokens":0,"totalTokens":0}}', 1788000000);
INSERT INTO api_usage_events VALUES
  ('request-1', 'private-user', '/v1/chat/completions', 'unsloth/local-api-model', 'completed', 20, 7, 27, 1788000100),
  ('request-2', 'private-user', '/v1/chat/completions', 'api-model', 'error', 50, 15, 65, 1783607400);
