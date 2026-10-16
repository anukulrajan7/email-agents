-- Slice S9.3/9.4: a sheet that lacks what the owner requires (a name, an employee ID, an email, a period) is not guessed at: the sender is
-- asked for exactly the missing items, and the answer is read when they reply in the thread. One row per sheet email, with the question
-- we asked, what is still missing, who answered, and where the exchange stands. Nobody is mailed more than three times.
CREATE TABLE info_requests (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  domain TEXT NOT NULL DEFAULT 'timesheet',
  thread_id TEXT NOT NULL,
  origin_message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  asked_to TEXT NOT NULL,
  asked_by_agent_id TEXT,
  -- [{key, label, status asked|answered|pending_owner|declined|conflict|filled, value, from_message_id, from_class, source, person_id, field, proposal}]
  fields_json TEXT NOT NULL DEFAULT '[]',
  state TEXT NOT NULL DEFAULT 'asking' CHECK (state IN ('asking', 'waiting', 'applying', 'waiting_owner', 'done', 'escalated', 'cancelled')),
  round INTEGER NOT NULL DEFAULT 1,
  reminders INTEGER NOT NULL DEFAULT 0,
  mails_sent INTEGER NOT NULL DEFAULT 0,
  next_at INTEGER,
  -- The Message-IDs our asks went out with: a reply that names one of them is the answer.
  ask_rfc_ids_json TEXT NOT NULL DEFAULT '[]',
  closed_reason TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX uq_info_requests_open ON info_requests(workspace_id, origin_message_id) WHERE state NOT IN ('done', 'escalated', 'cancelled');
CREATE INDEX idx_info_requests_thread ON info_requests(workspace_id, thread_id, state);
CREATE INDEX idx_info_requests_asked ON info_requests(workspace_id, asked_to, state);

ALTER TABLE timesheet_records ADD COLUMN info_request_id TEXT REFERENCES info_requests(id) ON DELETE SET NULL;

ALTER TABLE team_settings ADD COLUMN timesheet_required_json TEXT NOT NULL DEFAULT '["identity","period"]';
ALTER TABLE team_settings ADD COLUMN missing_reminder_hours INTEGER NOT NULL DEFAULT 24;
ALTER TABLE team_settings ADD COLUMN missing_escalate_hours INTEGER NOT NULL DEFAULT 48;
ALTER TABLE team_settings ADD COLUMN missing_max_reminders INTEGER NOT NULL DEFAULT 1;
