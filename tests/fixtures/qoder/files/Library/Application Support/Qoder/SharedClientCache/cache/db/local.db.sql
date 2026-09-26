-- 열은 TokenTracker src/lib/rollout.js:5777-5797의 쿼리가 읽는 것만 둔다
CREATE TABLE chat_message (id TEXT, session_id TEXT, request_id TEXT, role TEXT, token_info TEXT, model_info TEXT, gmt_create INTEGER);
CREATE TABLE chat_record (request_id TEXT, extra TEXT);
CREATE TABLE chat_session (session_id TEXT, preferred_model_info TEXT, project_uri TEXT, project_name TEXT);
INSERT INTO chat_session VALUES ('session-1', NULL, 'file:///work/app', 'app');
INSERT INTO chat_session VALUES ('session-2', '{"model_key":"quest-pro"}', 'file:///work/app', 'app');
INSERT INTO chat_record VALUES ('request-1', '{"modelConfig":{"key":"quest-ultimate"}}');
INSERT INTO chat_message VALUES ('user-1', 'session-1', 'request-1', 'user', '{"prompt_tokens":5,"cached_tokens":0,"completion_tokens":0}', NULL, 1784681690000);
INSERT INTO chat_message VALUES ('assistant-1', 'session-1', 'request-1', 'assistant', '{"prompt_tokens":56880,"cached_tokens":0,"completion_tokens":773}', '{"model_key":"quest-ultimate"}', 1784681696263);
INSERT INTO chat_message VALUES ('assistant-2', 'session-1', 'request-1', 'assistant', '{"prompt_tokens":57855,"cached_tokens":56878,"completion_tokens":186}', '{}', 1784681701844);
INSERT INTO chat_message VALUES ('assistant-3', 'session-2', 'request-2', 'assistant', '{"prompt_tokens":58299,"cached_tokens":57853,"completion_tokens":2812,"max_input_tokens":200000}', NULL, 1784681800000);
INSERT INTO chat_message VALUES ('assistant-4', 'session-2', 'request-3', 'assistant', NULL, NULL, 1784681900000);
