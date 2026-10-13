-- One row per model call (chat or embeddings). workspace_id is NULL when the caller had no context.
CREATE TABLE ai_calls (
  id TEXT PRIMARY KEY,
  workspace_id TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
  task TEXT NOT NULL,
  model TEXT NOT NULL,
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL NOT NULL DEFAULT 0,
  latency_ms INTEGER NOT NULL DEFAULT 0,
  ok INTEGER NOT NULL DEFAULT 1,
  error TEXT,
  message_id TEXT,
  case_id TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_ai_calls_workspace_created ON ai_calls(workspace_id, created_at);
CREATE INDEX idx_ai_calls_task ON ai_calls(task);
