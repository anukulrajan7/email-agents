-- Allow source = 'llm' (OpenRouter classifier). SQLite can't alter a CHECK, so rebuild.
-- Nothing references classifications, so the swap is safe inside the migration transaction.
CREATE TABLE classifications_new (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  category TEXT NOT NULL
    CHECK (category IN ('timesheet','leave','candidate_referral','approval','payroll','general','other')),
  priority TEXT NOT NULL CHECK (priority IN ('low','normal','high')),
  requires_reply INTEGER NOT NULL,
  confidence REAL,
  needs_review INTEGER NOT NULL DEFAULT 1,
  source TEXT NOT NULL CHECK (source IN ('jev','llm','manual','fallback')),
  model_id TEXT,
  model_version TEXT,
  taxonomy_version TEXT NOT NULL,
  prompt_version TEXT,
  updated_at INTEGER NOT NULL
);
INSERT INTO classifications_new SELECT * FROM classifications;
DROP TABLE classifications;
ALTER TABLE classifications_new RENAME TO classifications;
