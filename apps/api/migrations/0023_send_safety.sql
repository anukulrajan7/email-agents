-- Send safety: undo-able scheduled sends, transactional email outbox, judge verdict cache.

-- A draft is "scheduled" while send_at is set and it isn't sent yet (the API reports status
-- 'scheduled'). Kept as columns, not a new status value: widening the status CHECK would mean
-- rebuilding `drafts`, which other tables reference.
ALTER TABLE drafts ADD COLUMN send_at INTEGER;
-- Who asked for the send (user id); NULL = the system (automation/agent).
ALTER TABLE drafts ADD COLUMN send_requested_by TEXT;
-- Why the last scheduled send did not go out (blocked by the safety check, or Gmail failed).
ALTER TABLE drafts ADD COLUMN send_error TEXT;
-- Durable "sending" state: set right before Gmail is called, cleared only when Gmail clearly
-- rejected the send. While set (and status != 'sent') the outcome is unknown and must be
-- reconciled against Gmail (by our Message-ID / the thread) before any retry.
ALTER TABLE drafts ADD COLUMN send_attempt_id TEXT;
ALTER TABLE drafts ADD COLUMN send_started_at INTEGER;
CREATE INDEX idx_drafts_scheduled ON drafts(send_at) WHERE send_at IS NOT NULL;

-- Every OTP / alert email attempt. Subjects of verification emails are not stored (they hold the code).
CREATE TABLE email_outbox (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  to_address TEXT NOT NULL,
  subject TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('sending','sent','failed')),
  attempts INTEGER NOT NULL DEFAULT 0,
  last_error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_email_outbox_status ON email_outbox(status, created_at);

-- Typed judge answers keyed by a hash of (use, material, questions).
CREATE TABLE judge_cache (
  cache_key TEXT PRIMARY KEY,
  answers_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
