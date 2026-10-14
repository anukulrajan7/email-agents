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
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_designer_sessions_ws ON designer_sessions(workspace_id, updated_at DESC);

-- One row per designer request (a person's message, with any repair rounds), for the daily limit.
CREATE TABLE designer_calls (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  session_id TEXT NOT NULL REFERENCES designer_sessions(id) ON DELETE CASCADE,
  cost_usd REAL NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_designer_calls_day ON designer_calls(workspace_id, created_at);
