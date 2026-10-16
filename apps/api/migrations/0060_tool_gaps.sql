-- Tools agents wanted and we do not have: one row per workspace and tool name, counted each time it is asked for.
CREATE TABLE tool_gaps (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  tool TEXT NOT NULL,
  why TEXT NOT NULL DEFAULT '',
  agent_id TEXT REFERENCES agents(id) ON DELETE SET NULL,
  message_id TEXT REFERENCES email_messages(id) ON DELETE SET NULL,
  source TEXT NOT NULL CHECK (source IN ('designer', 'planner', 'rule', 'system')),
  count INTEGER NOT NULL DEFAULT 1,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'planned', 'built', 'dismissed')),
  first_seen_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  UNIQUE (workspace_id, tool)
);
