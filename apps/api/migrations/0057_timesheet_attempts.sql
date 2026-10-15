-- How many times the background scan has tried to read a timesheet email, so that mail the model could not read
-- (or that holds nothing to record) is not retried with model spend every 15 minutes. The scan stops after 3 tries.
CREATE TABLE timesheet_attempts (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  attempts INTEGER NOT NULL DEFAULT 0,
  last_at INTEGER NOT NULL
);
