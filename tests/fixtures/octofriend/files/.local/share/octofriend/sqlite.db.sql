-- Synthetic, from the Octofriend schema in research agents/octofriend.md section 2 (synthetic-lab/octofriend@9765799,
-- drizzle 0002 session history and 0006 history_items.model_json; LlmIR usage in source/libocto/llm-ir.ts:190-201).
-- ir 2: chat-completions compiler, total includes cache. ir 3: anthropic compiler, total is uncached input
-- (compilers/anthropic.ts:416-442). ir 4: a pre-0006 row with the literal backfill model value. ir 1 is a user turn.
CREATE TABLE trees (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, cwd TEXT, updated_at INTEGER);
CREATE TABLE llm_irs (id INTEGER PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE notifications (id INTEGER PRIMARY KEY, json TEXT);
CREATE TABLE history_items (id INTEGER PRIMARY KEY, llm_ir_id INTEGER, notification_id INTEGER,
  request_failed_id INTEGER, compaction_failed_id INTEGER, model_json TEXT NOT NULL);
CREATE TABLE tree_nodes (id INTEGER PRIMARY KEY, history_item_id INTEGER NOT NULL UNIQUE, tree_id INTEGER NOT NULL,
  parent_id INTEGER, is_leaf INTEGER, launch_id TEXT, created_at INTEGER);
INSERT INTO trees VALUES (1, '6f1c2d3e-0000-4000-8000-000000000001', '/work/p1', 1790000005000);
INSERT INTO notifications VALUES (1, '{"text":"update"}');
INSERT INTO llm_irs VALUES (1, '{"version":"octo-llm-ir/v1","ir":{"role":"user","content":"text"}}');
INSERT INTO llm_irs VALUES (2, '{"version":"octo-llm-ir/v1","ir":{"role":"assistant","content":"text","toolCalls":[],"usage":{"input":{"cached":8192,"uncached":1024,"total":9216},"output":312}}}');
INSERT INTO llm_irs VALUES (3, '{"version":"octo-llm-ir/v1","ir":{"role":"assistant","content":"text","toolCalls":[],"usage":{"input":{"cached":500,"uncached":-300,"total":200},"output":40}}}');
INSERT INTO llm_irs VALUES (4, '{"version":"octo-llm-ir/v1","ir":{"role":"assistant","content":"text","toolCalls":[],"usage":{"input":{"cached":0,"uncached":1000,"total":1000},"output":10}}}');
INSERT INTO history_items (id, llm_ir_id, notification_id, model_json) VALUES (1, 1, NULL, '{"version":"octo-model/v1","model":{"nickname":"big","model":"hf:deepseek-ai/DeepSeek-V3-0324","context":131072,"baseUrl":"https://api.synthetic.new/openai/v1","auth":{"type":"env","name":"SYNTHETIC_API_KEY"}}}');
INSERT INTO history_items (id, llm_ir_id, notification_id, model_json) VALUES (2, 2, NULL, '{"version":"octo-model/v1","model":{"nickname":"big","model":"hf:deepseek-ai/DeepSeek-V3-0324","context":131072,"baseUrl":"https://api.synthetic.new/openai/v1","auth":{"type":"env","name":"SYNTHETIC_API_KEY"}}}');
INSERT INTO history_items (id, llm_ir_id, notification_id, model_json) VALUES (3, 3, NULL, '{"version":"octo-model/v1","model":{"nickname":"claude","model":"claude-sonnet-4-6","context":200000,"type":"anthropic","baseUrl":"https://api.anthropic.com","auth":{"type":"command","command":["pass","show","anthropic"]}}}');
INSERT INTO history_items (id, llm_ir_id, notification_id, model_json) VALUES (4, 4, NULL, 'octo-no-model-recorded');
INSERT INTO history_items (id, llm_ir_id, notification_id, model_json) VALUES (5, NULL, 1, '{"version":"octo-model/v1","model":{"nickname":"big","model":"hf:deepseek-ai/DeepSeek-V3-0324","context":131072,"baseUrl":"https://api.synthetic.new/openai/v1","auth":{"type":"env","name":"SYNTHETIC_API_KEY"}}}');
INSERT INTO tree_nodes VALUES (1, 1, 1, NULL, 0, 'launch-1', 1790000001000);
INSERT INTO tree_nodes VALUES (2, 2, 1, 1, 0, 'launch-1', 1790000002000);
INSERT INTO tree_nodes VALUES (3, 3, 1, 2, 0, 'launch-1', 1790000003000);
INSERT INTO tree_nodes VALUES (4, 4, 1, 3, 0, 'launch-1', 1790000004000);
INSERT INTO tree_nodes VALUES (5, 5, 1, 4, 1, 'launch-1', 1790000005000);
