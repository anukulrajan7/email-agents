-- One agent per workspace (name, persona, jobs, permissions, quality tier) and a profile per
-- person: the agent adapts to whoever owns the mailbox an email arrived in.

-- No row = not set up yet; it is seeded on first read.
CREATE TABLE workspace_agents (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  -- One emoji or 1-2 letters.
  avatar TEXT NOT NULL,
  -- Palette name (same names as label colours).
  color TEXT NOT NULL,
  -- Team-written: who the agent is, how it talks, what it must never do. Trusted in prompts.
  persona TEXT NOT NULL DEFAULT '',
  signature TEXT,
  -- Jobs it does: "reply" (inbox agent), "requirements" (requirement agent).
  tasks_json TEXT NOT NULL DEFAULT '["reply","requirements"]',
  -- {"draft": bool, "send": "never" | "ask" | "auto"}. Submissions always ask.
  permissions_json TEXT NOT NULL DEFAULT '{"draft":true,"send":"ask"}',
  quality TEXT NOT NULL DEFAULT 'balanced' CHECK (quality IN ('fast','balanced','best')),
  enabled INTEGER NOT NULL DEFAULT 1,
  updated_by TEXT REFERENCES users(id),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- Written by each person about themselves; trusted in prompts for mail in their mailbox.
CREATE TABLE user_agent_profiles (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  display_name TEXT NOT NULL DEFAULT '',
  job_title TEXT NOT NULL DEFAULT '',
  handles_json TEXT NOT NULL DEFAULT '[]',
  handles_note TEXT NOT NULL DEFAULT '',
  -- What the agent should leave alone for them.
  ignore_note TEXT NOT NULL DEFAULT '',
  audiences_json TEXT NOT NULL DEFAULT '[]',
  -- Important addresses or domains.
  vips_json TEXT NOT NULL DEFAULT '[]',
  -- Tone: formality (casual|friendly|formal), length (short|medium|detailed), language, sign-off (signature).
  formality TEXT NOT NULL DEFAULT '',
  length TEXT NOT NULL DEFAULT '',
  language TEXT NOT NULL DEFAULT '',
  signature TEXT,
  working_hours TEXT NOT NULL DEFAULT '',
  timezone TEXT NOT NULL DEFAULT '',
  never_do TEXT NOT NULL DEFAULT '',
  -- Approval per action type: {"send_email": "always_ask" | "routine_ok"}. Submissions always ask.
  approvals_json TEXT NOT NULL DEFAULT '{"send_email":"always_ask"}',
  -- Onboarding: saved once, or skipped.
  saved_at INTEGER,
  skipped_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, user_id)
);

-- Whose mailbox the case's email arrived in (the person the agent works for).
ALTER TABLE cases ADD COLUMN agent_user_id TEXT REFERENCES users(id) ON DELETE SET NULL;

-- The agent may suggest an automation; a person approves it ('create_automation').
-- SQLite can't change a CHECK constraint in place, so the table is rebuilt.
CREATE TABLE agent_approvals_new (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  case_id TEXT NOT NULL REFERENCES cases(id) ON DELETE CASCADE,
  step_seq INTEGER,
  action TEXT NOT NULL CHECK (action IN ('send_email','submit_candidate','create_automation')),
  summary TEXT NOT NULL,
  to_address TEXT,
  subject TEXT,
  body TEXT NOT NULL DEFAULT '',
  draft_id TEXT REFERENCES drafts(id) ON DELETE SET NULL,
  requirement_id TEXT REFERENCES requirements(id) ON DELETE SET NULL,
  candidates_json TEXT NOT NULL DEFAULT '[]',
  input_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','declined')),
  decided_by TEXT REFERENCES users(id),
  decided_at INTEGER,
  note TEXT,
  created_at INTEGER NOT NULL
);
INSERT INTO agent_approvals_new (id, workspace_id, case_id, step_seq, action, summary, to_address, subject, body, draft_id,
                                 requirement_id, candidates_json, input_json, status, decided_by, decided_at, note, created_at)
  SELECT id, workspace_id, case_id, step_seq, action, summary, to_address, subject, body, draft_id,
         requirement_id, candidates_json, input_json, status, decided_by, decided_at, note, created_at
  FROM agent_approvals;
DROP TABLE agent_approvals;
ALTER TABLE agent_approvals_new RENAME TO agent_approvals;
CREATE INDEX idx_approvals_ws ON agent_approvals(workspace_id, status, created_at DESC);
CREATE INDEX idx_approvals_case ON agent_approvals(case_id, created_at);
CREATE UNIQUE INDEX uq_approvals_pending_case ON agent_approvals(case_id) WHERE status = 'pending';
