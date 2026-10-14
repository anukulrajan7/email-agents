-- What a model call really cost and how it ended: cache reads, who answered, why it stopped, whether the
-- answer parsed. Old rows keep the defaults. `cost_source` says if cost_usd is the provider's own number
-- ('provider', includes cache discounts), our price table ('table') or unknown ('none').
ALTER TABLE ai_calls ADD COLUMN cached_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ai_calls ADD COLUMN cache_write_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ai_calls ADD COLUMN reasoning_tokens INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ai_calls ADD COLUMN provider TEXT;
ALTER TABLE ai_calls ADD COLUMN answered_model TEXT;
ALTER TABLE ai_calls ADD COLUMN finish_reason TEXT;
ALTER TABLE ai_calls ADD COLUMN generation_id TEXT;
ALTER TABLE ai_calls ADD COLUMN prompt_version TEXT;
ALTER TABLE ai_calls ADD COLUMN schema_ok INTEGER;
ALTER TABLE ai_calls ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
ALTER TABLE ai_calls ADD COLUMN error_class TEXT;
ALTER TABLE ai_calls ADD COLUMN cost_source TEXT;
