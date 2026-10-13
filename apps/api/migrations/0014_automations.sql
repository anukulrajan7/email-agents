-- Automations: "when an email arrives and matches X, do Y". One run per (automation, message).
CREATE TABLE automations (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  trigger TEXT NOT NULL DEFAULT 'email_received',
  conditions_json TEXT NOT NULL DEFAULT '[]',
  actions_json TEXT NOT NULL DEFAULT '[]',
  preset_key TEXT,
  run_count INTEGER NOT NULL DEFAULT 0,
  last_run_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_automations_workspace ON automations(workspace_id, enabled, created_at);

CREATE TABLE automation_runs (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN ('completed','skipped','failed')),
  actions_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  UNIQUE (automation_id, message_id)
);
CREATE INDEX idx_automation_runs_automation ON automation_runs(automation_id, created_at DESC);
CREATE INDEX idx_automation_runs_workspace ON automation_runs(workspace_id, created_at DESC);

-- A workspace gets the default automations once; deleting them later is respected.
CREATE TABLE automation_seeds (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  seeded_at INTEGER NOT NULL
);

-- Review items can now come from an automation instead of a workflow run.
CREATE TABLE review_items_new (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  workflow_run_id TEXT REFERENCES workflow_runs(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','closed')),
  assigned_user_id TEXT REFERENCES users(id),
  created_at INTEGER NOT NULL,
  automation_id TEXT REFERENCES automations(id) ON DELETE CASCADE,
  UNIQUE(workflow_run_id, kind)
);
INSERT INTO review_items_new (id, workspace_id, message_id, workflow_run_id, kind, title, status, assigned_user_id, created_at)
SELECT id, workspace_id, message_id, workflow_run_id, kind, title, status, assigned_user_id, created_at FROM review_items;
DROP TABLE review_items;
ALTER TABLE review_items_new RENAME TO review_items;
CREATE INDEX idx_reviews_workspace ON review_items(workspace_id, status, created_at DESC);
CREATE UNIQUE INDEX uq_review_automation ON review_items(automation_id, message_id, kind) WHERE automation_id IS NOT NULL;
