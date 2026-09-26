-- TokenTracker test/claude-science-parser.test.js FRAMES_SCHEMA and REAL_FRAMES (:42-116, MIT (c) 2026 xiufengsun).
-- frame-1, frame-2, frame-empty :171-248; frame-aux :305-336; real-1..3 are REAL_FRAMES[1..3] from :254-303.
CREATE TABLE frames (
  id text PRIMARY KEY NOT NULL,
  parent_frame_id text,
  agent_name text NOT NULL DEFAULT 'agent',
  status text NOT NULL DEFAULT 'completed',
  model text,
  input_tokens integer,
  output_tokens integer,
  cache_read_tokens integer,
  cache_write_tokens integer,
  aux_input_tokens integer,
  aux_output_tokens integer,
  aux_cache_read_tokens integer,
  aux_cache_write_tokens integer,
  created_at integer NOT NULL,
  updated_at integer NOT NULL,
  completed_at integer,
  conversation_type text NOT NULL DEFAULT 'agent'
);
INSERT INTO frames (id, parent_frame_id, model, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
  aux_input_tokens, aux_output_tokens, created_at, updated_at, completed_at) VALUES
  ('frame-1', NULL, 'claude-opus-4-8', 5147303, 56725, 4873728, 165888, NULL, NULL, 1783429500000, 1783429500000, 1783429500000),
  ('frame-2', 'frame-1', 'claude-opus-4-8', 20, 10, 0, 0, NULL, NULL, 1783431600000, 1783431600000, 1783431600000),
  ('frame-empty', NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, 1783431660000, 1783431660000, NULL),
  ('frame-aux', NULL, 'claude-opus-4-8', 5147303, 56725, 4873728, 165888, 259081, 35707, 1783429500000, 1783429500000, 1783429500000),
  ('real-1', NULL, 'claude-opus-4-8', 7040691, 65640, 6720000, 210432, NULL, NULL, 1783429500000, 1783429500000, 1783429500000),
  ('real-2', NULL, 'claude-opus-4-8', 8251244, 82224, 7754752, 372224, NULL, NULL, 1783429500000, 1783429500000, 1783429500000),
  ('real-3', NULL, 'claude-opus-4-8', 3146490, 47778, 2908160, 156672, NULL, NULL, 1783429500000, 1783429500000, 1783429500000);
