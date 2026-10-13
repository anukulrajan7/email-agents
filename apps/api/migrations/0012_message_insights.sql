-- "Understand before replying": what the sender asks, deadline, whether a reply is needed.
CREATE TABLE message_insights (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  summary TEXT NOT NULL,
  asks_json TEXT NOT NULL DEFAULT '[]',
  deadline TEXT,
  needs_reply INTEGER NOT NULL,
  missing_json TEXT NOT NULL DEFAULT '[]',
  source TEXT NOT NULL CHECK (source IN ('llm','rules')),
  model_id TEXT,
  created_at INTEGER NOT NULL
);
-- Asks the draft could not answer from known facts (shown as [NEEDS: …] to the person).
ALTER TABLE drafts ADD COLUMN unanswered_json TEXT NOT NULL DEFAULT '[]';
