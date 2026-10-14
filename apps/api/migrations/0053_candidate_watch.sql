-- Job seekers told "we'll send matching roles": a new strong match emails them (weekly cap, STOP opts out).
-- `email` is the canonical form of their address (plus-tags and Gmail dots removed).
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
CREATE INDEX idx_candidate_watch_candidate ON candidate_watch(candidate_id);

-- A confidential requirement never names its client to anyone outside the team.
ALTER TABLE requirements ADD COLUMN confidential INTEGER NOT NULL DEFAULT 0;
