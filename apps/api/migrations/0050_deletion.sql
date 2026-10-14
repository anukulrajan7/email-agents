-- Deleting mail from Post Room (the provider keeps its copy).
--
-- A deleted message leaves a tombstone so the next sync of the same mailbox does not import it again.
-- It is keyed by the provider's own message id and has no foreign key, so it survives the message.
CREATE TABLE deleted_messages (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL,
  provider_message_id TEXT NOT NULL,
  deleted_by TEXT,
  deleted_at INTEGER NOT NULL,
  PRIMARY KEY (source_id, provider_message_id)
);

-- Disconnecting a mailbox purges what came from it in the background. NULL = never requested;
-- 'running' while batches are deleted; 'done' once the source-scoped rows are removed as well.
ALTER TABLE email_sources ADD COLUMN purge_status TEXT;
ALTER TABLE email_sources ADD COLUMN purge_total INTEGER;
ALTER TABLE email_sources ADD COLUMN purge_done INTEGER;
ALTER TABLE email_sources ADD COLUMN purge_counts_json TEXT;
ALTER TABLE email_sources ADD COLUMN purged_at INTEGER;
