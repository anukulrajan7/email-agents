-- Per-agent metrics (a rollup of cases, approvals, drafts, routes, audit and the AI ledger), in-product
-- eval runs with per-case results, and the log of autonomy changes (promotions and demotions).
-- Allowed values are enforced in Rust.

CREATE TABLE agent_metrics_daily (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  agent_id TEXT NOT NULL,
  agent_version INTEGER NOT NULL,
  -- days since the Unix epoch, UTC
  day INTEGER NOT NULL,
  emails_routed INTEGER NOT NULL DEFAULT 0,
  cases_started INTEGER NOT NULL DEFAULT 0,
  cases_done INTEGER NOT NULL DEFAULT 0,
  cases_failed INTEGER NOT NULL DEFAULT 0,
  cases_cancelled INTEGER NOT NULL DEFAULT 0,
  approvals_requested INTEGER NOT NULL DEFAULT 0,
  approvals_approved INTEGER NOT NULL DEFAULT 0,
  approvals_edited INTEGER NOT NULL DEFAULT 0,
  approvals_declined INTEGER NOT NULL DEFAULT 0,
  drafts_created INTEGER NOT NULL DEFAULT 0,
  drafts_sent INTEGER NOT NULL DEFAULT 0,
  edit_distance_sum REAL NOT NULL DEFAULT 0,
  edit_distance_n INTEGER NOT NULL DEFAULT 0,
  auto_sends INTEGER NOT NULL DEFAULT 0,
  auto_sends_stopped INTEGER NOT NULL DEFAULT 0,
  incidents INTEGER NOT NULL DEFAULT 0,
  label_corrections INTEGER NOT NULL DEFAULT 0,
  labels_total INTEGER NOT NULL DEFAULT 0,
  shadow_agreement_sum REAL NOT NULL DEFAULT 0,
  shadow_agreement_n INTEGER NOT NULL DEFAULT 0,
  cost_usd REAL NOT NULL DEFAULT 0,
  -- email received to case done
  latency_p50_ms INTEGER,
  latency_p95_ms INTEGER,
  PRIMARY KEY (workspace_id, agent_id, agent_version, day)
);
CREATE INDEX idx_metrics_daily_agent ON agent_metrics_daily(workspace_id, agent_id, day DESC);

CREATE TABLE eval_runs (
  id TEXT PRIMARY KEY,
  workspace_id TEXT,
  agent_id TEXT,
  agent_version INTEGER,
  -- safety|replay|golden_live|learning_lift
  suite TEXT NOT NULL,
  -- version|schedule|manual|promotion
  trigger TEXT NOT NULL,
  -- queued|running|passed|failed|error
  status TEXT NOT NULL,
  score REAL,
  cases_total INTEGER NOT NULL DEFAULT 0,
  cases_passed INTEGER NOT NULL DEFAULT 0,
  baseline_run_id TEXT,
  cost_usd REAL NOT NULL DEFAULT 0,
  summary_json TEXT NOT NULL DEFAULT '{}',
  requested_by TEXT,
  started_at INTEGER,
  finished_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_eval_runs_agent ON eval_runs(workspace_id, agent_id, created_at DESC);

CREATE TABLE eval_results (
  run_id TEXT NOT NULL REFERENCES eval_runs(id) ON DELETE CASCADE,
  case_key TEXT NOT NULL,
  passed INTEGER NOT NULL,
  score REAL,
  detail_json TEXT NOT NULL DEFAULT '{}',
  PRIMARY KEY (run_id, case_key)
);

CREATE TABLE autonomy_changes (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  from_level INTEGER NOT NULL,
  to_level INTEGER NOT NULL,
  reason TEXT NOT NULL,
  evidence_json TEXT NOT NULL DEFAULT '{}',
  -- user:<id> or system
  actor TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_autonomy_changes_agent ON autonomy_changes(workspace_id, agent_id, created_at DESC);
