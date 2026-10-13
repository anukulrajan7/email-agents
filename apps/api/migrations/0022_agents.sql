-- Agents: durable cases worked step by step by a planner, with a full trace and human approvals.

CREATE TABLE cases (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('requirement','inbox')),
  title TEXT NOT NULL,
  goal TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'running'
    CHECK (status IN ('running','waiting_approval','waiting_event','done','failed','cancelled')),
  outcome TEXT CHECK (outcome IS NULL OR outcome IN ('submitted','interview','rejected','placed','closed')),
  status_note TEXT,
  failure_reason TEXT,
  state_json TEXT NOT NULL DEFAULT '{}',
  priority TEXT NOT NULL DEFAULT 'normal' CHECK (priority IN ('low','normal','high')),
  -- Planner turns taken (one durable job each); compared with max_steps.
  step_count INTEGER NOT NULL DEFAULT 0,
  max_steps INTEGER NOT NULL,
  spent_usd REAL NOT NULL DEFAULT 0,
  budget_usd REAL NOT NULL,
  -- Consecutive turns that ended in an error; 3 in a row fails the case.
  error_streak INTEGER NOT NULL DEFAULT 0,
  message_id TEXT REFERENCES email_messages(id) ON DELETE SET NULL,
  requirement_id TEXT REFERENCES requirements(id) ON DELETE SET NULL,
  thread_id TEXT REFERENCES email_threads(id) ON DELETE SET NULL,
  next_wake_at INTEGER,
  created_by TEXT REFERENCES users(id),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_cases_ws ON cases(workspace_id, status, updated_at DESC);
CREATE INDEX idx_cases_thread ON cases(thread_id, status);
CREATE UNIQUE INDEX uq_cases_requirement ON cases(workspace_id, kind, requirement_id) WHERE requirement_id IS NOT NULL;
CREATE UNIQUE INDEX uq_cases_message ON cases(workspace_id, kind, message_id) WHERE message_id IS NOT NULL;

CREATE TABLE case_steps (
  id TEXT PRIMARY KEY,
  case_id TEXT NOT NULL REFERENCES cases(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  turn INTEGER NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('tool','critic','approval','note','finish')),
  tool TEXT,
  summary TEXT NOT NULL,
  input_json TEXT NOT NULL DEFAULT 'null',
  output_json TEXT NOT NULL DEFAULT 'null',
  status TEXT NOT NULL CHECK (status IN ('ok','error','blocked','waiting')),
  tokens_in INTEGER NOT NULL DEFAULT 0,
  tokens_out INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL NOT NULL DEFAULT 0,
  -- Internal trace only; never returned by the API.
  model_id TEXT,
  created_at INTEGER NOT NULL,
  UNIQUE (case_id, seq)
);

CREATE TABLE agent_approvals (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  case_id TEXT NOT NULL REFERENCES cases(id) ON DELETE CASCADE,
  step_seq INTEGER,
  action TEXT NOT NULL CHECK (action IN ('send_email','submit_candidate')),
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
CREATE INDEX idx_approvals_ws ON agent_approvals(workspace_id, status, created_at DESC);
CREATE INDEX idx_approvals_case ON agent_approvals(case_id, created_at);
CREATE UNIQUE INDEX uq_approvals_pending_case ON agent_approvals(case_id) WHERE status = 'pending';
