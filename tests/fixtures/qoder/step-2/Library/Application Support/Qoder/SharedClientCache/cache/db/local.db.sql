-- token_info는 나중에 붙는다(TokenTracker rollout.js:6052-6110)
UPDATE chat_message SET token_info = '{"prompt_tokens":100,"cached_tokens":80,"completion_tokens":10}' WHERE id = 'assistant-4';
