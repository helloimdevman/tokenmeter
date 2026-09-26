-- ZCode ~/.zcode/cli/db/db.sqlite model_usage 표(ccusage·tokscale fixture 열의 합집합). 행 하나가 API 시도 하나다.
-- usage_1: tokscale sessions/zcode.rs:851-909, usage_cache_incl: :1032-1068, usage_cache_excl: :1177-1214
-- (MIT © tokscale authors; status·provider_id·started_at을 더하고 usage_cache_incl의 모델을 GLM-5.2로).
-- usage-1/usage-2: ccusage rust/adapters/zcode/src/loader.rs:360-414 (MIT © 2025 ryoppippi; 작업 폴더만 바꿈).
-- usage_anth: 합성(TokenTracker rollout.js:5256-5285 하위 에이전트 거르기).
CREATE TABLE model_usage (
  id TEXT PRIMARY KEY, session_id TEXT, turn_id TEXT, logical_request_id TEXT, attempt_index INTEGER,
  model_id TEXT, provider_id TEXT, status TEXT, started_at INTEGER, completed_at INTEGER, duration_ms INTEGER,
  input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
  cache_read_input_tokens INTEGER, cache_creation_input_tokens INTEGER, computed_total_tokens INTEGER,
  agent TEXT, mode TEXT);
CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT, version TEXT);
INSERT INTO session VALUES
  ('sess_1', '/work/zcode', '0.16.3'), ('sess_cache', '/work/zcode', '0.16.3'),
  ('sess_excl', '/work/zcode', '0.16.3'), ('session-1', '/work/zcode', '0.16.3');
INSERT INTO model_usage (id, session_id, turn_id, model_id, provider_id, status, started_at, completed_at, duration_ms,
                         input_tokens, output_tokens, reasoning_tokens, cache_read_input_tokens,
                         cache_creation_input_tokens, computed_total_tokens, agent, mode) VALUES
  -- 포함형(total = input + output): input에서 캐시 둘을 빼고 output은 추론을 이미 담는다 → 90/7/3/20
  ('usage_1', 'sess_1', 'turn_1', 'GLM-5.2', 'builtin:zai-coding-plan', 'completed', 1782718000000, 1782718001000, 1000,
   100, 20, 5, 7, 3, 120, 'zcode-agent', 'yolo'),
  -- 포함형 → 15/80/5/50
  ('usage_cache_incl', 'sess_cache', NULL, 'GLM-5.2', 'builtin:bigmodel-coding-plan', 'completed', 1782718002000, 1782718003000, 1000,
   100, 50, 10, 80, 5, 150, NULL, NULL),
  -- 배타형(total = 다섯 칸의 합): 그대로, 추론은 output에 더함 → 20/80/10/35. 사용자 프로바이더(UUID)는 센다
  ('usage_cache_excl', 'sess_excl', NULL, 'claude-sonnet-5', '6f1c2e0a-3b4d-4e5f-8a9b-0c1d2e3f4a5b', 'completed', 1782718004000, 1782718005000, 1000,
   20, 30, 5, 80, 10, 145, NULL, NULL),
  -- ccusage 두 행: completed만 센다 → 60/25/15/10. running은 step-2에서 끝난다
  ('usage-1', 'session-1', NULL, 'GLM-5.3', 'builtin:zai-coding-plan', 'completed', 1786909042666, NULL, NULL,
   100, 10, NULL, 25, 15, 110, NULL, NULL),
  ('usage-2', 'session-1', NULL, 'GLM-5.3', 'builtin:zai-coding-plan', 'running', 1786909042666, NULL, NULL,
   0, 0, NULL, 0, 0, 0, NULL, NULL),
  -- 함께 든 Claude Code 하위 에이전트: 자기 ~/.claude 로그가 따로 있다 → 세지 않음
  ('usage_anth', 'sess_1', 'turn_1', 'claude-sonnet-4-6', 'anthropic', 'completed', 1782718006000, 1782718007000, 1000,
   50, 5, 0, 0, 0, 55, NULL, NULL);
