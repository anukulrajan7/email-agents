-- Header evidence that a message is bulk or automated (JSON array of signal names, see src/bulk.rs).
ALTER TABLE email_messages ADD COLUMN bulk_signals TEXT NOT NULL DEFAULT '[]';

-- Email understanding v2: the full verified result next to the insight columns it fills (see ai/understand.rs).
ALTER TABLE message_insights ADD COLUMN understanding_json TEXT;
ALTER TABLE message_insights ADD COLUMN understanding_version TEXT;

-- Shadow mode (UNDERSTAND_V2=shadow): production labelling ran as usual and v2 ran beside it. One row per email.
CREATE TABLE understanding_shadow (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  v1_label TEXT NOT NULL,
  v2_label TEXT,                 -- NULL when the v2 call failed
  label_agree INTEGER NOT NULL DEFAULT 0,
  v1_requires_reply INTEGER NOT NULL,
  v2_needs_reply INTEGER,
  v1_priority TEXT NOT NULL,
  v2_priority TEXT,
  v2_model TEXT,
  error TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_understanding_shadow_ws ON understanding_shadow(workspace_id, created_at);
