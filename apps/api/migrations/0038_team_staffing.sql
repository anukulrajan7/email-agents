-- Team and staffing operations (WP-G).
-- Generic (every workspace): team_settings, workspace_owners, people, person_emails, alerts.person_id.
-- HR & recruiting pack only: submissions, submission_scans, portal_hours, portal_submissions, portal_mismatches.
-- No existing table is rebuilt: the super admin is a separate table, not a new memberships.role value.

CREATE TABLE team_settings (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  -- Local time for "today" and "this week" (minutes east of UTC).
  tz_offset_minutes INTEGER NOT NULL DEFAULT 0 CHECK (tz_offset_minutes BETWEEN -840 AND 840),
  -- Hours an alert waits at the owner, then at the manager, before it moves up. NULL = alert_settings.escalate_after_hours.
  manager_after_hours INTEGER CHECK (manager_after_hours IS NULL OR manager_after_hours BETWEEN 1 AND 720),
  final_after_hours INTEGER CHECK (final_after_hours IS NULL OR final_after_hours BETWEEN 1 AND 720),
  -- Last escalation level (HR / admins). Empty = the alert recipients.
  final_emails_json TEXT NOT NULL DEFAULT '[]',
  below_target_hour INTEGER NOT NULL DEFAULT 16 CHECK (below_target_hour BETWEEN 0 AND 23),
  weekly_check_hour INTEGER NOT NULL DEFAULT 12 CHECK (weekly_check_hour BETWEEN 0 AND 23),
  -- Staffing pack only.
  hr_source_id TEXT REFERENCES email_sources(id) ON DELETE SET NULL,
  client_domains_json TEXT NOT NULL DEFAULT '[]',
  timesheet_grace_days INTEGER NOT NULL DEFAULT 3 CHECK (timesheet_grace_days BETWEEN 0 AND 28),
  hours_tolerance REAL NOT NULL DEFAULT 0.5 CHECK (hours_tolerance >= 0),
  updated_at INTEGER NOT NULL
);

-- The super admin (HRMS owner): one per workspace, always an admin member.
CREATE TABLE workspace_owners (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id),
  created_at INTEGER NOT NULL
);
INSERT OR IGNORE INTO workspace_owners (workspace_id, user_id, created_at)
SELECT workspace_id, user_id, CAST(strftime('%s', 'now') AS INTEGER) * 1000
FROM memberships
WHERE role = 'admin' AND rowid IN (SELECT MIN(rowid) FROM memberships WHERE role = 'admin' GROUP BY workspace_id);

CREATE TABLE people (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  -- Staffing pack only; every other workspace keeps the default.
  employee_type TEXT NOT NULL DEFAULT 'non_billable' CHECK (employee_type IN ('billable','non_billable')),
  client TEXT,
  manager_id TEXT REFERENCES people(id) ON DELETE SET NULL,
  role TEXT,
  -- YYYY-MM-DD
  joining_date TEXT,
  daily_target INTEGER CHECK (daily_target IS NULL OR daily_target BETWEEN 0 AND 1000),
  weekly_target INTEGER CHECK (weekly_target IS NULL OR weekly_target BETWEEN 0 AND 5000),
  -- The person's own connected mailbox.
  source_id TEXT REFERENCES email_sources(id) ON DELETE SET NULL,
  active INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_people_ws ON people(workspace_id, active);
CREATE INDEX idx_people_manager ON people(manager_id);
CREATE INDEX idx_people_source ON people(source_id);

CREATE TABLE person_emails (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  email TEXT NOT NULL,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  position INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (workspace_id, email)
);
CREATE INDEX idx_person_emails_person ON person_emails(person_id, position);

-- The owner of an alert, for the owner -> manager -> final escalation chain.
ALTER TABLE alerts ADD COLUMN person_id TEXT;

-- Submissions: a recruiter's sent mail to a client with a CV attached.
CREATE TABLE submissions (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  thread_id TEXT NOT NULL,
  sent_at INTEGER NOT NULL,
  client_domain TEXT NOT NULL,
  client TEXT,
  attachment_name TEXT NOT NULL,
  -- Sorted name tokens from the CV file name: the identity of the candidate within a thread.
  candidate_key TEXT NOT NULL,
  candidate_name TEXT,
  candidate_id TEXT REFERENCES candidates(id) ON DELETE SET NULL,
  requirement_id TEXT REFERENCES requirements(id) ON DELETE SET NULL,
  -- auto = linked with confidence; needs_confirm = a person must choose; confirmed = a person chose; unlinked = nothing found.
  link_status TEXT NOT NULL DEFAULT 'unlinked' CHECK (link_status IN ('auto','needs_confirm','confirmed','unlinked')),
  link_options_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','dismissed')),
  outcome TEXT NOT NULL DEFAULT 'pending' CHECK (outcome IN ('pending','interview','rejected','selected','joined')),
  reached_interview INTEGER NOT NULL DEFAULT 0,
  outcome_message_id TEXT,
  outcome_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (person_id, thread_id, candidate_key)
);
CREATE INDEX idx_submissions_person ON submissions(workspace_id, person_id, sent_at);
CREATE INDEX idx_submissions_review ON submissions(workspace_id, link_status);

-- One row per outbound message already looked at, so the scan never repeats work.
CREATE TABLE submission_scans (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  result TEXT NOT NULL,
  scanned_at INTEGER NOT NULL
);

-- Portal data (CSV import).
CREATE TABLE portal_hours (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  month TEXT NOT NULL,
  hours REAL NOT NULL CHECK (hours >= 0),
  imported_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, person_id, month)
);

CREATE TABLE portal_submissions (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  day TEXT NOT NULL,
  month TEXT NOT NULL,
  candidate TEXT NOT NULL,
  candidate_key TEXT NOT NULL,
  client TEXT,
  imported_at INTEGER NOT NULL,
  UNIQUE (workspace_id, person_id, day, candidate_key)
);
CREATE INDEX idx_portal_submissions_month ON portal_submissions(workspace_id, month, person_id);

CREATE TABLE portal_mismatches (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  person_id TEXT NOT NULL REFERENCES people(id) ON DELETE CASCADE,
  month TEXT NOT NULL,
  kind TEXT NOT NULL,
  detail_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','resolved')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (workspace_id, person_id, month, kind)
);
CREATE INDEX idx_portal_mismatches_ws ON portal_mismatches(workspace_id, status, month);
