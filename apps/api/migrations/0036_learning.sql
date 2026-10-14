-- Learning loop: every user action becomes an append-only signal, learned style preferences live apart
-- from facts (memories), memories can belong to one agent, and edit distance is stored on drafts.
-- Allowed values (kind, category, status, source) are enforced in Rust.

CREATE TABLE feedback_events (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  agent_id TEXT,
  agent_version INTEGER,
  actor_user_id TEXT,
  owner_user_id TEXT,
  kind TEXT NOT NULL,
  message_id TEXT,
  draft_id TEXT,
  case_id TEXT,
  approval_id TEXT,
  -- 0 = unchanged, 1 = rewritten (normalised word-level Levenshtein)
  edit_distance REAL,
  -- Labels and agent ids only; never bodies.
  before_json TEXT NOT NULL DEFAULT '{}',
  after_json TEXT NOT NULL DEFAULT '{}',
  note TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_feedback_agent ON feedback_events(workspace_id, agent_id, created_at DESC);
CREATE INDEX idx_feedback_kind ON feedback_events(workspace_id, kind, created_at DESC);

CREATE TABLE agent_preferences (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  -- NULL agent = all agents; NULL user = whole team
  agent_id TEXT,
  user_id TEXT,
  -- greeting|sign_off|length|tone|structure|phrasing|process (Rust-validated)
  category TEXT NOT NULL,
  content TEXT NOT NULL,
  -- suggested|active|archived (archived = tombstone, never relearned)
  status TEXT NOT NULL DEFAULT 'suggested',
  -- edit|decline_note|manual
  source TEXT NOT NULL,
  dedupe_key TEXT NOT NULL,
  evidence_count INTEGER NOT NULL DEFAULT 1,
  evidence_json TEXT NOT NULL DEFAULT '[]',
  use_count INTEGER NOT NULL DEFAULT 0,
  embedding BLOB,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX uq_prefs_dedupe ON agent_preferences(workspace_id, COALESCE(agent_id, ''), COALESCE(user_id, ''), dedupe_key);
CREATE INDEX idx_prefs_lookup ON agent_preferences(workspace_id, status, agent_id);

-- NULL = shared team memory. A personal agent's memories carry its id and are visible to its owner only.
ALTER TABLE memories ADD COLUMN agent_id TEXT;
CREATE INDEX idx_memories_agent ON memories(workspace_id, agent_id);

ALTER TABLE drafts ADD COLUMN edit_distance REAL;
