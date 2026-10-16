-- Per-workspace switches for behaviour changes: off, compare (run beside today's and log the difference) or on.
-- A missing row means the default from Config, which is off in production.
CREATE TABLE workspace_flags (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  mode TEXT NOT NULL CHECK (mode IN ('off', 'compare', 'on')),
  updated_by TEXT REFERENCES users(id),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, key)
);
