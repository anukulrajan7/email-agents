-- Slice S7: one gate for mail to a correspondent. An expert (Rohan) files a typed request of what must be said; Meera's
-- reply job merges the requests of a message into one reply. One row per (message, requesting agent, job): filing the same
-- request again changes nothing, a changed one replaces the unsent row.
CREATE TABLE message_requests (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  from_agent_id TEXT NOT NULL,
  job TEXT NOT NULL,
  from_case_id TEXT,
  kind TEXT NOT NULL CHECK (kind IN ('ack','correction','question','answer','custom','holding')),
  recipient_json TEXT NOT NULL DEFAULT '{"to":"sender"}',
  facts_json TEXT NOT NULL DEFAULT '[]',
  issues_json TEXT NOT NULL DEFAULT '[]',
  instructions TEXT,
  never_do_json TEXT NOT NULL DEFAULT '[]',
  body TEXT,
  requester_level INTEGER NOT NULL DEFAULT 0,
  digest TEXT NOT NULL,
  expires_at INTEGER,
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','waiting','composed','sent','superseded','dropped','cancelled')),
  status_note TEXT,
  reply_case_id TEXT,
  draft_id TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (workspace_id, message_id, from_agent_id, job)
);
CREATE INDEX idx_message_requests_message ON message_requests(workspace_id, message_id, status);
CREATE INDEX idx_message_requests_draft ON message_requests(draft_id) WHERE draft_id IS NOT NULL;

-- Which experts asked for this draft: [{agent_id, name, kind, job, level}]. Data only for now (metrics and blame come later).
ALTER TABLE drafts ADD COLUMN requested_by_json TEXT NOT NULL DEFAULT '[]';
