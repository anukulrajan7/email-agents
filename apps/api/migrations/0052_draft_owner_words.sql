-- What a person themselves said or typed for a draft (Ask Meera turns, instructions, their own edits), one row per
-- author and text. A send is supported only by its own author's words.
CREATE TABLE draft_owner_words (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  draft_id TEXT NOT NULL REFERENCES drafts(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL,
  text TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE (draft_id, user_id, text)
);
