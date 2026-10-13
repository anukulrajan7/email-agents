-- Command assistant: actions the planner proposes wait here for one person's confirmation.
-- Each row is bound to the user who asked, expires after 30 minutes and can be used once.
CREATE TABLE assistant_plans (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id),
  action TEXT NOT NULL,
  summary TEXT NOT NULL,
  params_json TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  used_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_assistant_plans_expiry ON assistant_plans(expires_at);
