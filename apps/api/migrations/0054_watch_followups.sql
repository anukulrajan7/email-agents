-- A person confirmed this requirement is real: candidates on watch may be told about it.
ALTER TABLE requirements ADD COLUMN confirmed_at INTEGER;

-- One hand-off task per draft, however many sends race to create it.
CREATE TABLE handoff_marks (
  draft_id TEXT PRIMARY KEY REFERENCES drafts(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL
);
