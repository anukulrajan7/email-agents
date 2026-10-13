-- No row = defaults. auto_reply NULL = "follow the server default (AUTO_DRAFTS)".
CREATE TABLE workspace_reply_settings (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  auto_reply INTEGER,
  reply_instructions TEXT NOT NULL DEFAULT '',
  signature TEXT,
  updated_at INTEGER NOT NULL
);
