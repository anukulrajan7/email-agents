-- Timesheets kept as records. A record is the data read from one timesheet (pasted in the email or attached),
-- linked to the email and the person. It must outlive the email: deleting the email or disconnecting its mailbox
-- never deletes a record, so message_id and attachment_id are SET NULL and the record keeps a snapshot of the
-- sender, subject, date and file name.
ALTER TABLE people ADD COLUMN employee_code TEXT;
CREATE UNIQUE INDEX idx_people_employee_code ON people(workspace_id, lower(employee_code)) WHERE employee_code IS NOT NULL;

-- Links a person chose by hand ("this code / name is that person"); the next sheet with it matches automatically.
CREATE TABLE timesheet_aliases (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('code', 'name')),
  value TEXT NOT NULL,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, kind, value)
);

CREATE TABLE timesheet_records (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  person_id TEXT REFERENCES people(id) ON DELETE SET NULL,
  -- Identity as written in the sheet or mail.
  employee_code TEXT,
  person_name TEXT,
  sender_address TEXT,
  identity_json TEXT NOT NULL DEFAULT '{}',
  match_method TEXT NOT NULL DEFAULT 'none' CHECK (match_method IN ('code', 'alias', 'email', 'name', 'manual', 'none')),
  match_confidence REAL,
  -- The email and file it came from, plus what they said, kept after either is deleted.
  message_id TEXT REFERENCES email_messages(id) ON DELETE SET NULL,
  attachment_id TEXT REFERENCES attachments(id) ON DELETE SET NULL,
  source_key TEXT NOT NULL,
  source TEXT NOT NULL CHECK (source IN ('pasted', 'attachment')),
  email_subject TEXT,
  email_received_at INTEGER,
  attachment_filename TEXT,
  attachment_kind TEXT,
  period_start TEXT NOT NULL,
  period_end TEXT NOT NULL,
  total_hours REAL,
  entries_json TEXT NOT NULL DEFAULT '[]',
  client TEXT,
  status TEXT NOT NULL CHECK (status IN ('recorded', 'replaced', 'needs_check')),
  replaces_id TEXT,
  issues_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX idx_timesheet_records_source ON timesheet_records(workspace_id, source_key);
CREATE INDEX idx_timesheet_records_person ON timesheet_records(workspace_id, person_id, period_start);
CREATE INDEX idx_timesheet_records_period ON timesheet_records(workspace_id, period_start);
CREATE INDEX idx_timesheet_records_message ON timesheet_records(message_id) WHERE message_id IS NOT NULL;
