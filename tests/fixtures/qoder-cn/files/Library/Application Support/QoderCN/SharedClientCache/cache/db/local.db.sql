-- 열은 TokenTracker src/lib/rollout.js:5777-5797의 쿼리가 읽는 것만 둔다
CREATE TABLE chat_message (id TEXT, session_id TEXT, request_id TEXT, role TEXT, token_info TEXT, model_info TEXT, gmt_create INTEGER);
CREATE TABLE chat_record (request_id TEXT, extra TEXT);
CREATE TABLE chat_session (session_id TEXT, preferred_model_info TEXT, project_uri TEXT, project_name TEXT);
INSERT INTO chat_message VALUES ('cn-assistant-1', 'cn-session-1', 'cn-request-1', 'assistant', '{"prompt_tokens":56880,"cached_tokens":0,"completion_tokens":773}', '{"model_key":"quest-ultimate"}', 1784681696263);
