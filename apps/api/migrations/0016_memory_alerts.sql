-- Self-learning memory (auto facts with confidence, sources, pinning, usage) and the alerts engine.

-- origin: 'manual' = written by a person; 'auto' = learned by the system (from emails, replies or draft edits).
-- The API's `source_message_id` is the existing `evidence_message_id` (latest source); `sources_json` keeps
-- up to 10 sources as [{"id": message_id, "via": "incoming"|"reply"}].
ALTER TABLE memories ADD COLUMN origin TEXT NOT NULL DEFAULT 'manual' CHECK (origin IN ('auto','manual'));
ALTER TABLE memories ADD COLUMN confidence REAL NOT NULL DEFAULT 1.0;
ALTER TABLE memories ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN use_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN last_used_at INTEGER;
-- Who/what an auto fact is about (an email address, a company, 'team') and its normalised
-- "subject|key" identity: a new fact with the same key updates the existing memory.
ALTER TABLE memories ADD COLUMN subject TEXT;
ALTER TABLE memories ADD COLUMN dedupe_key TEXT;
ALTER TABLE memories ADD COLUMN sources_json TEXT NOT NULL DEFAULT '[]';

UPDATE memories SET origin = 'auto' WHERE source = 'learned';
UPDATE memories SET confidence = 0.5 WHERE source = 'learned' AND status = 'suggested';
UPDATE memories SET sources_json = json_array(json_object('id', evidence_message_id, 'via', 'reply'))
WHERE evidence_message_id IS NOT NULL;

CREATE UNIQUE INDEX uq_memories_dedupe ON memories(workspace_id, dedupe_key) WHERE dedupe_key IS NOT NULL;
CREATE INDEX idx_memories_eviction ON memories(workspace_id, origin, status, pinned, use_count, confidence);

-- Alerts: one row per (workspace, dedupe_key); dedupe_key = 'kind:message_id:level' (level the alert was raised at).
CREATE TABLE alerts (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  severity TEXT NOT NULL CHECK (severity IN ('high','normal')),
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  message_id TEXT REFERENCES email_messages(id) ON DELETE CASCADE,
  dedupe_key TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','snoozed','resolved')),
  escalation_level INTEGER NOT NULL DEFAULT 1,
  -- Highest level whose email went out; an open alert with notified_level < escalation_level is pending delivery.
  notified_level INTEGER NOT NULL DEFAULT 0,
  notified_json TEXT NOT NULL DEFAULT '[]',
  snoozed_until INTEGER,
  -- When the current escalation level started (the escalation clock).
  level_since INTEGER NOT NULL,
  resolved_at INTEGER,
  resolved_by TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (workspace_id, dedupe_key)
);
CREATE INDEX idx_alerts_ws ON alerts(workspace_id, status, created_at DESC);
CREATE INDEX idx_alerts_message ON alerts(message_id);

-- No row = defaults. recipients_json NULL = "the workspace's admins".
CREATE TABLE alert_settings (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  enabled INTEGER NOT NULL DEFAULT 1,
  recipients_json TEXT,
  escalate_to_json TEXT NOT NULL DEFAULT '[]',
  unanswered_hours INTEGER NOT NULL DEFAULT 4,
  escalate_after_hours INTEGER NOT NULL DEFAULT 24,
  daily_digest INTEGER NOT NULL DEFAULT 1,
  digest_hour INTEGER NOT NULL DEFAULT 9 CHECK (digest_hour BETWEEN 0 AND 23),
  -- Days since the Unix epoch (UTC) of the last digest; claims the digest once per day.
  last_digest_day INTEGER,
  updated_at INTEGER NOT NULL
);
