-- Job seekers we told "we'll send matching roles": a new matching requirement emails them (at most weekly; STOP opts out).
CREATE TABLE candidate_watch (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  candidate_id TEXT NOT NULL REFERENCES candidates(id) ON DELETE CASCADE,
  email TEXT NOT NULL,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  opted_out INTEGER NOT NULL DEFAULT 0,
  last_notified_at INTEGER,
  created_at INTEGER NOT NULL,
  UNIQUE (workspace_id, email)
);

-- A confidential requirement never names its client to anyone outside the team.
ALTER TABLE requirements ADD COLUMN confidential INTEGER NOT NULL DEFAULT 0;
