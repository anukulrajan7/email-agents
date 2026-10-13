-- Core schema. WAL is enabled at connect time, never inside a migration transaction.

CREATE TABLE workspaces (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  created_at INTEGER NOT NULL
);

CREATE TABLE users (
  id TEXT PRIMARY KEY,
  external_auth_subject TEXT NOT NULL UNIQUE,
  email TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_users_email ON users(email);

CREATE TABLE memberships (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id),
  role TEXT NOT NULL CHECK (role IN ('admin','operator')),
  PRIMARY KEY (workspace_id, user_id)
);
CREATE INDEX idx_memberships_user ON memberships(user_id);

CREATE TABLE email_sources (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('forwarding','nango_gmail','nango_microsoft','future')),
  status TEXT NOT NULL CHECK (status IN ('pending','connected','degraded','disconnected')),
  inbound_alias TEXT UNIQUE,
  external_connection_id TEXT,
  capabilities_json TEXT NOT NULL DEFAULT '{}',
  last_synced_at INTEGER,
  created_at INTEGER NOT NULL,
  UNIQUE(kind, external_connection_id)
);
CREATE INDEX idx_sources_workspace ON email_sources(workspace_id);
CREATE UNIQUE INDEX uq_forwarding_per_workspace ON email_sources(workspace_id) WHERE kind = 'forwarding';

CREATE TABLE inbound_events (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  provider_event_id TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'received' CHECK (status IN ('received','processed','rejected')),
  created_at INTEGER NOT NULL,
  UNIQUE(provider, provider_event_id)
);

CREATE TABLE email_threads (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL REFERENCES email_sources(id),
  provider_thread_id TEXT,
  subject TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX uq_source_thread ON email_threads(source_id, provider_thread_id)
  WHERE provider_thread_id IS NOT NULL;

CREATE TABLE email_messages (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL REFERENCES email_sources(id),
  thread_id TEXT NOT NULL REFERENCES email_threads(id),
  source_message_id TEXT NOT NULL,
  rfc_message_id TEXT,
  sender_address TEXT NOT NULL,
  original_sender_address TEXT,
  recipients_json TEXT NOT NULL DEFAULT '[]',
  subject TEXT NOT NULL,
  body_text TEXT NOT NULL DEFAULT '',
  body_html TEXT,
  received_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  processing_status TEXT NOT NULL DEFAULT 'pending'
    CHECK (processing_status IN ('pending','normalized','classified','workflow_completed','failed')),
  processing_error TEXT,
  UNIQUE(source_id, source_message_id)
);
CREATE INDEX idx_inbox ON email_messages(workspace_id, received_at DESC, id DESC);
CREATE INDEX idx_thread_messages ON email_messages(thread_id, received_at);

CREATE TABLE attachments (
  id TEXT PRIMARY KEY,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  filename TEXT NOT NULL,
  media_type TEXT,
  size_bytes INTEGER,
  storage_key TEXT,
  scan_status TEXT NOT NULL DEFAULT 'pending'
);
CREATE INDEX idx_attachments_message ON attachments(message_id);

CREATE TABLE jobs (
  id TEXT PRIMARY KEY,
  workspace_id TEXT REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  dedupe_key TEXT NOT NULL UNIQUE,
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending'
    CHECK (status IN ('pending','running','succeeded','dead')),
  attempts INTEGER NOT NULL DEFAULT 0,
  max_attempts INTEGER NOT NULL DEFAULT 5,
  run_at INTEGER NOT NULL,
  locked_until INTEGER,
  locked_by TEXT,
  last_error TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_claim_jobs ON jobs(status, run_at, locked_until);
CREATE INDEX idx_jobs_workspace ON jobs(workspace_id, status);

CREATE TABLE classifications (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  category TEXT NOT NULL
    CHECK (category IN ('timesheet','leave','candidate_referral','approval','payroll','general','other')),
  priority TEXT NOT NULL CHECK (priority IN ('low','normal','high')),
  requires_reply INTEGER NOT NULL,
  confidence REAL,
  needs_review INTEGER NOT NULL DEFAULT 1,
  source TEXT NOT NULL CHECK (source IN ('jev','manual','fallback')),
  model_id TEXT,
  model_version TEXT,
  taxonomy_version TEXT NOT NULL,
  prompt_version TEXT,
  updated_at INTEGER NOT NULL
);

CREATE TABLE drafts (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  body_text TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('suggested','edited','approved','sent')),
  model_id TEXT,
  origin_key TEXT UNIQUE,
  provider_draft_id TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_drafts_message ON drafts(message_id, created_at DESC);

CREATE TABLE workflows (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  template_key TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 0,
  config_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  UNIQUE(workspace_id, template_key)
);

CREATE TABLE workflow_runs (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  workflow_id TEXT NOT NULL REFERENCES workflows(id),
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  status TEXT NOT NULL CHECK (status IN ('running','completed','failed','skipped')),
  error TEXT,
  created_at INTEGER NOT NULL,
  UNIQUE(workflow_id, message_id)
);
CREATE INDEX idx_runs_workspace ON workflow_runs(workspace_id, created_at DESC);

CREATE TABLE review_items (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  workflow_run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','closed')),
  assigned_user_id TEXT REFERENCES users(id),
  created_at INTEGER NOT NULL,
  UNIQUE(workflow_run_id, kind)
);
CREATE INDEX idx_reviews_workspace ON review_items(workspace_id, status, created_at DESC);

CREATE TABLE audit_events (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  actor_type TEXT NOT NULL CHECK (actor_type IN ('user','system')),
  actor_id TEXT,
  action TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  message_id TEXT,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_audit_message ON audit_events(workspace_id, message_id, created_at);
