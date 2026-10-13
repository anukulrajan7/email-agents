-- Knowledge store v2: a content hash per indexed document (idempotent re-indexing), `kind` as a
-- vec0 metadata column (KNN filtered by kind, e.g. only past replies), and vectors removed
-- together with their chunk (including FK cascades from messages and workspaces).

ALTER TABLE knowledge_chunks ADD COLUMN content_hash TEXT NOT NULL DEFAULT '';

DROP TABLE knowledge_vec;
CREATE VIRTUAL TABLE knowledge_vec USING vec0(
  workspace_id TEXT PARTITION KEY,
  kind TEXT,
  embedding FLOAT[1024] distance_metric=cosine
);
UPDATE knowledge_chunks SET embedded = 0;

CREATE TRIGGER knowledge_vec_ad AFTER DELETE ON knowledge_chunks BEGIN
  DELETE FROM knowledge_vec WHERE rowid = old.id;
END;
