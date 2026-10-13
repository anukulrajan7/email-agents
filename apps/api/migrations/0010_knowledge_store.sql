-- Retrieval layer: chunks with keyword (FTS5) and vector (sqlite-vec, bge-m3 1024-d) indexes,
-- scoped per workspace. Plus AI-vs-human label tracking for evaluation and few-shot examples.

CREATE TABLE knowledge_chunks (
  id INTEGER PRIMARY KEY,               -- stable rowid shared with the FTS and vector tables
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('email','attachment','reply')),
  ref_id TEXT NOT NULL,                 -- message / attachment / draft id
  message_id TEXT REFERENCES email_messages(id) ON DELETE CASCADE,
  position INTEGER NOT NULL DEFAULT 0,
  text TEXT NOT NULL,
  embedded INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_chunks_ref ON knowledge_chunks(workspace_id, kind, ref_id);
CREATE INDEX idx_chunks_message ON knowledge_chunks(message_id);
CREATE INDEX idx_chunks_pending ON knowledge_chunks(embedded, workspace_id);

CREATE VIRTUAL TABLE knowledge_fts USING fts5(text, content='knowledge_chunks', content_rowid='id', tokenize='porter unicode61');
CREATE TRIGGER knowledge_fts_ai AFTER INSERT ON knowledge_chunks BEGIN
  INSERT INTO knowledge_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER knowledge_fts_ad AFTER DELETE ON knowledge_chunks BEGIN
  INSERT INTO knowledge_fts(knowledge_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE VIRTUAL TABLE knowledge_vec USING vec0(
  workspace_id TEXT PARTITION KEY,
  embedding FLOAT[1024] distance_metric=cosine
);

-- What the AI first said (kept when a human changes it) and whether a human confirmed it.
ALTER TABLE classifications ADD COLUMN ai_category TEXT;
ALTER TABLE classifications ADD COLUMN confirmed INTEGER NOT NULL DEFAULT 0;
UPDATE classifications SET ai_category = category WHERE source IN ('llm','jev','fallback');
UPDATE classifications SET confirmed = 1 WHERE source = 'manual';
