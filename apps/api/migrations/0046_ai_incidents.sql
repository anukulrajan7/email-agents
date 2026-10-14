-- Things that went wrong or nearly did around AI: a blocked leak, an injection attempt, a refusal, a
-- malformed answer, a claim the context did not support, a blocked send, a cost or latency spike.
-- `details_json` never holds email bodies. `dedupe_key` makes a retried job record one incident, not many.
CREATE TABLE ai_incidents (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  agent_id TEXT,
  case_id TEXT,
  message_id TEXT,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL DEFAULT 'low',
  details_json TEXT NOT NULL DEFAULT '{}',
  dedupe_key TEXT,
  created_at INTEGER NOT NULL,
  resolved_at INTEGER,
  resolved_by TEXT
);
CREATE UNIQUE INDEX uq_ai_incidents_dedupe ON ai_incidents(workspace_id, dedupe_key) WHERE dedupe_key IS NOT NULL;
CREATE INDEX idx_ai_incidents_ws_created ON ai_incidents(workspace_id, created_at DESC);
CREATE INDEX idx_ai_incidents_open ON ai_incidents(workspace_id, resolved_at, created_at DESC);
CREATE INDEX idx_ai_incidents_agent ON ai_incidents(agent_id, created_at);
