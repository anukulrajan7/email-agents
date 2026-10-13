-- Agent memory: learned contact profiles + user/learned notes with full-text search.

CREATE TABLE contacts (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  email TEXT NOT NULL,
  display_name TEXT,
  message_count INTEGER NOT NULL DEFAULT 0,
  first_seen_at INTEGER NOT NULL,
  last_seen_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, email)
);
CREATE INDEX idx_contacts_recent ON contacts(workspace_id, last_seen_at DESC);
-- Per-sender category history is computed from classifications on demand.
CREATE INDEX idx_messages_sender ON email_messages(workspace_id, sender_address);

CREATE TABLE memories (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  scope TEXT NOT NULL CHECK (scope IN ('workspace','contact')),
  contact_email TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('fact','preference','style')),
  content TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active','suggested','archived')),
  source TEXT NOT NULL CHECK (source IN ('user','learned')),
  created_by TEXT REFERENCES users(id),
  evidence_message_id TEXT REFERENCES email_messages(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK ((scope = 'contact') = (contact_email IS NOT NULL))
);
CREATE INDEX idx_memories_lookup ON memories(workspace_id, status, scope, contact_email, updated_at DESC);

-- Self-contained FTS index keyed by memory_id (not rowid, which VACUUM may renumber).
CREATE VIRTUAL TABLE memories_fts USING fts5(memory_id UNINDEXED, content, tokenize = 'porter unicode61');
CREATE TRIGGER memories_fts_insert AFTER INSERT ON memories BEGIN
  INSERT INTO memories_fts(memory_id, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER memories_fts_update AFTER UPDATE OF content ON memories BEGIN
  DELETE FROM memories_fts WHERE memory_id = old.id;
  INSERT INTO memories_fts(memory_id, content) VALUES (new.id, new.content);
END;
CREATE TRIGGER memories_fts_delete AFTER DELETE ON memories BEGIN
  DELETE FROM memories_fts WHERE memory_id = old.id;
END;

-- What each draft was given, and the original suggestion (to learn from operator edits).
ALTER TABLE drafts ADD COLUMN memory_ids_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE drafts ADD COLUMN original_body_text TEXT;

-- Backfill contacts from existing mail.
INSERT INTO contacts (workspace_id, email, display_name, message_count, first_seen_at, last_seen_at)
SELECT workspace_id, sender_address, NULL, COUNT(*), MIN(received_at), MAX(received_at)
FROM email_messages WHERE sender_address != '' GROUP BY workspace_id, sender_address;
