-- 자리표시 행을 최종값으로, 경계 행 db-msg-1은 time_updated만 다시 올린다(스펙 13절 꼭 둘 step-2)
UPDATE message SET time_updated = 1767313200000,
  data = '{"role":"assistant","providerID":"anthropic","modelID":"claude-sonnet-4-20250514","time":{"created":1767312300000,"completed":1767312309000},"tokens":{"input":40,"output":60,"reasoning":10,"cache":{"read":500,"write":0}},"cost":0.02}'
  WHERE id = 'db-msg-6';
UPDATE message SET time_updated = 1767313200000 WHERE id = 'db-msg-1';
