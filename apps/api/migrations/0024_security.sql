-- Security hardening: pending invitations, password-reset codes, persistent rate limits,
-- and an index for the global daily AI budget.

-- An admin invited an email that has no verified account yet. The membership is created
-- when that person signs up and verifies the address.
CREATE TABLE invitations (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  email TEXT NOT NULL,
  role TEXT NOT NULL CHECK (role IN ('admin','operator')),
  invited_by TEXT REFERENCES users(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, email)
);
CREATE INDEX idx_invitations_email ON invitations(email);

-- email_codes.purpose gains 'password_reset' (SQLite can't alter a CHECK: rebuild).
CREATE TABLE email_codes_new (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  purpose TEXT NOT NULL CHECK (purpose IN ('verify_email','password_reset')),
  code_hash TEXT NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  consumed_at INTEGER
);
INSERT INTO email_codes_new (id, user_id, purpose, code_hash, attempts, created_at, expires_at, consumed_at)
SELECT id, user_id, purpose, code_hash, attempts, created_at, expires_at, consumed_at FROM email_codes;
DROP TABLE email_codes;
ALTER TABLE email_codes_new RENAME TO email_codes;
CREATE INDEX idx_email_codes_user ON email_codes(user_id, purpose, created_at DESC);

-- Sliding-window limits that must survive restarts (test emails, alert tests, reset codes).
CREATE TABLE rate_events (
  scope TEXT NOT NULL,
  key TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_rate_events ON rate_events(scope, key, created_at);

CREATE INDEX idx_ai_calls_created ON ai_calls(created_at);
