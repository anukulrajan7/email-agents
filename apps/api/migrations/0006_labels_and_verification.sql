-- Workspace-defined labels replace the fixed HR category list; email verification codes.

CREATE TABLE labels (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL,
  color TEXT NOT NULL DEFAULT 'ink'
    CHECK (color IN ('slate','sage','plum','ochre','teal','postmark','ink','rule')),
  position INTEGER NOT NULL DEFAULT 0,
  system INTEGER NOT NULL DEFAULT 0,
  archived INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  UNIQUE (workspace_id, key)
);
CREATE INDEX idx_labels_workspace ON labels(workspace_id, archived, position);

-- Existing workspaces were HR workspaces: give them the HR label set.
INSERT INTO labels (id, workspace_id, key, name, description, color, position, system, created_at)
SELECT 'lbl_' || lower(hex(randomblob(16))), w.id, t.key, t.name, t.description, t.color, t.position, t.system,
       CAST(strftime('%s','now') AS INTEGER) * 1000
FROM workspaces w
CROSS JOIN (
  SELECT 'timesheet' AS key, 'Timesheet' AS name, 'An employee or contractor submits, corrects or asks about a timesheet or hours worked.' AS description, 'slate' AS color, 0 AS position, 0 AS system
  UNION ALL SELECT 'leave', 'Leave', 'A request, approval or question about leave, vacation, sick days or time off.', 'sage', 1, 0
  UNION ALL SELECT 'candidate_referral', 'Candidate referral', 'Someone refers or recommends a job candidate, or sends a CV for a referral.', 'plum', 2, 0
  UNION ALL SELECT 'approval', 'Approval', 'A request for a manager or HR approval that is not leave or timesheet related.', 'ochre', 3, 0
  UNION ALL SELECT 'payroll', 'Payroll', 'Salary, payslips, reimbursements, tax forms or payment issues.', 'teal', 4, 0
  UNION ALL SELECT 'general', 'General HR', 'A legitimate HR question or notice that fits no specific label above.', 'ink', 5, 0
  UNION ALL SELECT 'other', 'Other', 'Spam, newsletters, automated notifications, or anything that fits no other label.', 'rule', 6, 1
) t;

-- classifications.category is now a label key validated by the application.
CREATE TABLE classifications_new (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  category TEXT NOT NULL,
  priority TEXT NOT NULL CHECK (priority IN ('low','normal','high')),
  requires_reply INTEGER NOT NULL,
  confidence REAL,
  needs_review INTEGER NOT NULL DEFAULT 1,
  source TEXT NOT NULL CHECK (source IN ('jev','llm','manual','fallback')),
  model_id TEXT,
  model_version TEXT,
  taxonomy_version TEXT NOT NULL,
  prompt_version TEXT,
  updated_at INTEGER NOT NULL
);
INSERT INTO classifications_new SELECT * FROM classifications;
DROP TABLE classifications;
ALTER TABLE classifications_new RENAME TO classifications;
CREATE INDEX idx_classifications_category ON classifications(category);

-- Email verification (password accounts). Existing accounts are treated as verified.
ALTER TABLE users ADD COLUMN email_verified_at INTEGER;
UPDATE users SET email_verified_at = created_at;

CREATE TABLE email_codes (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  purpose TEXT NOT NULL CHECK (purpose IN ('verify_email')),
  code_hash TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER
);
CREATE INDEX idx_email_codes_user ON email_codes(user_id, purpose, created_at DESC);
