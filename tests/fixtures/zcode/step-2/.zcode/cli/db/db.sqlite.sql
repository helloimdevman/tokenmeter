-- running 시도가 끝나 최종 토큰으로 고쳐 쓰인다(ccusage loader.rs:360-374의 값) → 한 번만 더해진다
UPDATE model_usage SET status = 'completed', completed_at = 1786909043666, duration_ms = 1000,
  input_tokens = 100, output_tokens = 10, cache_read_input_tokens = 25, cache_creation_input_tokens = 15,
  computed_total_tokens = 110
  WHERE id = 'usage-2';
