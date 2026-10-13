-- Ask Meera is one persona, so each person has one continuous conversation with her per workspace.
-- Rows are private to the user who wrote them; the API only ever reads by (workspace_id, user_id).
-- citations_json / plan_json hold what the answer showed (plan cards keep their assistant_plans ids,
-- so a reopened card knows whether it was confirmed or has expired).
CREATE TABLE assistant_messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  role TEXT NOT NULL CHECK (role IN ('user', 'assistant')),
  text TEXT NOT NULL,
  citations_json TEXT NOT NULL DEFAULT '[]',
  plan_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_assistant_messages_owner ON assistant_messages(workspace_id, user_id, id DESC);
