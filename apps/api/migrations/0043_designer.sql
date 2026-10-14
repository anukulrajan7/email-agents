-- System agent designer: a conversation that turns a plain-English job into a validated AgentSpec,
-- a dry-run report over recent mail, and (when saved) a new agent or a new version of one.
-- Allowed values are enforced in Rust.

CREATE TABLE designer_sessions (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id),
  -- Set when the session edits an existing agent; base_version is the version it started from.
  agent_id TEXT,
  base_version INTEGER,
  goal TEXT NOT NULL,
  -- [{role: user|designer, text}], at most 20
  turns_json TEXT NOT NULL DEFAULT '[]',
  spec_json TEXT,
  issues_json TEXT NOT NULL DEFAULT '[]',
  dry_run_json TEXT,
  -- idle|running|done|failed
  dry_run_status TEXT NOT NULL DEFAULT 'idle',
  -- drafting|ready|saved|abandoned
  status TEXT NOT NULL DEFAULT 'drafting',
  cost_usd REAL NOT NULL DEFAULT 0,
  -- Bumped by every update; an update that read an older value is refused (compare and swap).
  rev INTEGER NOT NULL DEFAULT 0,
  -- Hash of the spec the running or finished dry run belongs to.
  dry_run_spec_hash TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_designer_sessions_ws ON designer_sessions(workspace_id, updated_at DESC);

-- One row per designer request (a person's message, with any repair rounds) or dry run. The row is reserved
-- before the model is asked, so the limits hold under concurrent requests.
CREATE TABLE designer_calls (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  session_id TEXT NOT NULL REFERENCES designer_sessions(id) ON DELETE CASCADE,
  user_id TEXT,
  -- design (counts against the workspace's daily limit) | personal (a member's own agent, per-member limit) | dry_run
  kind TEXT NOT NULL DEFAULT 'design',
  -- pending (reserved, model not answered yet) | done | failed
  status TEXT NOT NULL DEFAULT 'pending',
  cost_usd REAL NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_designer_calls_day ON designer_calls(workspace_id, created_at);
