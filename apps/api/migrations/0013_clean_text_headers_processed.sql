-- Wave 2 (agent A): clean stored text, Gmail-style headers, Gmail HTML refetch, 'processed' status.

-- Sender display name and Cc list, so the UI can render a Gmail-style header.
ALTER TABLE email_messages ADD COLUMN sender_name TEXT;
ALTER TABLE email_messages ADD COLUMN cc_json TEXT NOT NULL DEFAULT '[]';

-- Mail the connected mailbox itself sent (Gmail SENT label, or sender = the mailbox address). Stored as
-- evidence of replies; never classified, analysed, drafted against or shown in the default inbox.
-- Existing rows are marked outbound by the Gmail sync once it has learned the mailbox address.
ALTER TABLE email_messages ADD COLUMN direction TEXT NOT NULL DEFAULT 'inbound' CHECK (direction IN ('inbound','outbound'));
ALTER TABLE email_sources ADD COLUMN mailbox_address TEXT;
CREATE INDEX idx_messages_thread_direction ON email_messages(thread_id, direction, received_at);

-- 0 = body_text predates text cleaning; the `clean_stored_text` job re-cleans it and sets 1.
-- New rows are cleaned at ingest and take the default (1).
ALTER TABLE email_messages ADD COLUMN text_clean_v INTEGER NOT NULL DEFAULT 1;
UPDATE email_messages SET text_clean_v = 0;

-- Counts attempts to re-fetch the HTML part of Gmail mail stored without one. New rows default to
-- "done" (99); only legacy Gmail rows with no HTML are queued (0), at most 3 tries each.
ALTER TABLE email_messages ADD COLUMN html_refetch_tries INTEGER NOT NULL DEFAULT 99;
UPDATE email_messages SET html_refetch_tries = 0
WHERE body_html IS NULL AND content_complete = 1
  AND source_id IN (SELECT id FROM email_sources WHERE kind = 'nango_gmail');

-- Terminal states 'workflow_running' and 'processed'. SQLite cannot alter a CHECK, and rebuilding
-- email_messages would cascade-delete every child row (foreign keys are on), so the constraint text
-- is edited in place (the documented writable_schema procedure; no row data changes).
PRAGMA writable_schema = ON;
UPDATE sqlite_master
SET sql = replace(sql, '''classified'',''workflow_completed'',''failed''',
                       '''classified'',''workflow_running'',''workflow_completed'',''processed'',''failed''')
WHERE type = 'table' AND name = 'email_messages';
-- Any schema change bumps the schema cookie, so other connections reload the edited definition.
CREATE TABLE _schema_bump (x INTEGER);
DROP TABLE _schema_bump;
-- RESET (instead of OFF) also reparses the schema on this connection, which would otherwise keep the
-- old CHECK in memory.
PRAGMA writable_schema = RESET;
