-- Slice S9.2: nobody who mails us waits in silence. One row per inbound mail a desk owns: a deadline in working hours, and what met it
-- (a sent reply, a person's own answer) or why no reply is owed (an excuse, with its reason). The timers are jobs (REPLY_SLA) and an hourly
-- sweep; the row holds when each stage ran so that none runs twice.
CREATE TABLE reply_obligations (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  thread_id TEXT NOT NULL,
  agent_id TEXT,
  job TEXT NOT NULL,
  sender_address TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'sent', 'holding_sent', 'excused', 'answered_by_person', 'escalated', 'bounced', 'cancelled')),
  excuse TEXT,
  warn_at INTEGER NOT NULL,
  due_at INTEGER NOT NULL,
  final_due_at INTEGER,
  warned_at INTEGER,
  held_at INTEGER,
  escalated_at INTEGER,
  reply_draft_id TEXT,
  closed_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (workspace_id, message_id)
);
CREATE INDEX idx_reply_obligations_open ON reply_obligations(workspace_id, status, due_at);
CREATE INDEX idx_reply_obligations_thread ON reply_obligations(workspace_id, thread_id, status);

ALTER TABLE team_settings ADD COLUMN timezone TEXT;
ALTER TABLE team_settings ADD COLUMN reply_sla_hours INTEGER NOT NULL DEFAULT 4;
ALTER TABLE team_settings ADD COLUMN work_start_hour INTEGER NOT NULL DEFAULT 9;
ALTER TABLE team_settings ADD COLUMN work_end_hour INTEGER NOT NULL DEFAULT 19;
ALTER TABLE team_settings ADD COLUMN work_days_json TEXT NOT NULL DEFAULT '[0,1,2,3,4,5]';
ALTER TABLE team_settings ADD COLUMN holding_mode TEXT NOT NULL DEFAULT 'auto' CHECK (holding_mode IN ('auto', 'off'));
-- Addresses ("ravi@x.com") and domains ("@x.com") the owner never wants an automatic reply to (until owner rules, S16, replace it).
ALTER TABLE team_settings ADD COLUMN no_reply_json TEXT NOT NULL DEFAULT '[]';
UPDATE team_settings SET timezone = 'Asia/Kolkata' WHERE tz_offset_minutes IN (0, 330);
