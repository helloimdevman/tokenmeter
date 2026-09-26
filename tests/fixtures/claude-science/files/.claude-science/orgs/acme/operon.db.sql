-- Pre-rename file name under a multi-org install, with the schema before the aux_* migration:
-- TokenTracker test/claude-science-parser.test.js:338-372 (legacy-1 = REAL_FRAMES[0]). MIT (c) 2026 xiufengsun.
CREATE TABLE frames (
  id text PRIMARY KEY NOT NULL,
  parent_frame_id text,
  model text,
  input_tokens integer,
  output_tokens integer,
  cache_read_tokens integer,
  cache_write_tokens integer,
  created_at integer NOT NULL,
  updated_at integer NOT NULL,
  completed_at integer
);
INSERT INTO frames VALUES ('legacy-1', NULL, 'claude-opus-4-8', 5147303, 56725, 4873728, 165888, 1783429500000, 1783429500000, 1783429500000);
