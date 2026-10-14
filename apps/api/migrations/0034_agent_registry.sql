-- Agent registry: many agents per workspace (a lead, system desks seeded from packs, custom agents
-- and one personal agent per member), their versions, message routes and per-agent case ownership.
-- Nothing is rebuilt: only tables and columns are added; allowed values are enforced in Rust.

CREATE TABLE agents (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('lead','system','custom')),
  -- workspace | personal (Rust-validated). A personal agent belongs to one member and is invisible to everyone else.
  scope TEXT NOT NULL DEFAULT 'workspace',
  owner_user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
  -- general|support|sales|finance|personal|hrms|custom (Rust-validated)
  category TEXT NOT NULL,
  template_key TEXT,
  template_version INTEGER,
  cloned_from TEXT,
  name TEXT NOT NULL,
  avatar TEXT NOT NULL,
  color TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  -- draft|active|paused|archived (Rust-validated)
  status TEXT NOT NULL DEFAULT 'draft',
  -- pack_off|user|demoted_incident|pause_all
  paused_reason TEXT,
  active_version INTEGER,
  tuned INTEGER NOT NULL DEFAULT 0,
  -- Earned level 0..3 (written only through agent_registry::store::set_autonomy_level).
  autonomy_level INTEGER NOT NULL DEFAULT 0,
  -- Ceiling an admin grants (the owner, for a personal agent).
  autonomy_cap INTEGER NOT NULL DEFAULT 1,
  next_run_at INTEGER,
  created_by TEXT REFERENCES users(id),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (workspace_id, key)
);
CREATE UNIQUE INDEX uq_agents_lead ON agents(workspace_id) WHERE kind = 'lead';
CREATE UNIQUE INDEX uq_agents_personal ON agents(workspace_id, owner_user_id) WHERE scope = 'personal';
CREATE INDEX idx_agents_ws ON agents(workspace_id, status, category);

CREATE TABLE agent_versions (
  agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  spec_json TEXT NOT NULL,
  spec_hash TEXT NOT NULL,
  -- template|tuned|designer|manual|rollback|backfill
  source TEXT NOT NULL,
  designer_session_id TEXT,
  note TEXT,
  -- pending|passed|failed|overridden
  gate_status TEXT NOT NULL DEFAULT 'pending',
  -- {automation_id: version}
  automation_pins_json TEXT NOT NULL DEFAULT '{}',
  created_by TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (agent_id, version)
);

CREATE TABLE automation_versions (
  automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  definition_json TEXT NOT NULL,
  created_by TEXT,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (automation_id, version)
);

CREATE TABLE message_routes (
  message_id TEXT PRIMARY KEY REFERENCES email_messages(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  agent_version INTEGER NOT NULL,
  -- manual|thread|rule|label|personal|generic|fallback
  source TEXT NOT NULL,
  shadow INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_routes_agent ON message_routes(workspace_id, agent_id, created_at DESC);

ALTER TABLE cases ADD COLUMN agent_id TEXT;
ALTER TABLE cases ADD COLUMN agent_version INTEGER;
ALTER TABLE cases ADD COLUMN "trigger" TEXT;
ALTER TABLE cases ADD COLUMN outcome_detail TEXT;
ALTER TABLE cases ADD COLUMN shadow INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_cases_agent ON cases(workspace_id, agent_id, created_at DESC);

-- One real inbox case per message (as before); an observing (shadow) agent may run alongside it,
-- once per agent.
DROP INDEX uq_cases_message;
CREATE UNIQUE INDEX uq_cases_message ON cases(workspace_id, kind, message_id) WHERE message_id IS NOT NULL AND shadow = 0;
CREATE UNIQUE INDEX uq_cases_shadow ON cases(workspace_id, agent_id, message_id) WHERE message_id IS NOT NULL AND shadow = 1;

ALTER TABLE agent_approvals ADD COLUMN agent_id TEXT;
ALTER TABLE agent_approvals ADD COLUMN approved_hash TEXT;
ALTER TABLE automations ADD COLUMN agent_id TEXT;
ALTER TABLE automations ADD COLUMN version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE drafts ADD COLUMN agent_id TEXT;
CREATE INDEX idx_drafts_agent_send ON drafts(agent_id, send_at) WHERE send_at IS NOT NULL;

-- Backfill: every workspace gets its lead agent. The lead's name and look come from the existing
-- workspace agent (which stays the lead's persona store); a workspace that lets routine replies go
-- starts at level 2, everyone else at level 1, so today's behaviour is unchanged.
INSERT INTO agents (id, workspace_id, key, kind, scope, owner_user_id, category, template_key, template_version, name, avatar, color,
                    description, status, active_version, tuned, autonomy_level, autonomy_cap, created_at, updated_at)
SELECT 'agt_' || lower(hex(randomblob(16))), w.id, 'lead', 'lead', 'workspace', NULL, 'general', 'general.lead', 1,
       COALESCE(wa.name, 'Meera'), COALESCE(wa.avatar, '🌼'), COALESCE(wa.color, 'ochre'),
       'Routes mail, answers what no desk owns and keeps the team''s voice.', 'active', NULL, 0,
       CASE WHEN CASE WHEN json_valid(wa.permissions_json) THEN json_extract(wa.permissions_json, '$.send') END = 'auto' THEN 2 ELSE 1 END,
       CASE WHEN CASE WHEN json_valid(wa.permissions_json) THEN json_extract(wa.permissions_json, '$.send') END = 'auto' THEN 2 ELSE 1 END,
       CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000
FROM workspaces w LEFT JOIN workspace_agents wa ON wa.workspace_id = w.id;

-- Every member gets one personal agent that works on their own mailbox.
INSERT INTO agents (id, workspace_id, key, kind, scope, owner_user_id, category, template_key, template_version, name, avatar, color,
                    description, status, active_version, tuned, autonomy_level, autonomy_cap, created_by, created_at, updated_at)
SELECT 'agt_' || lower(hex(randomblob(16))), m.workspace_id, 'me_' || m.user_id, 'system', 'personal', m.user_id, 'personal',
       'personal.mailbox', 1, 'My assistant', '🙋', 'sage',
       'Works on your own mailbox, in your voice, using your profile and preferences.', 'active', NULL, 0,
       CASE WHEN CASE WHEN json_valid(wa.permissions_json) THEN json_extract(wa.permissions_json, '$.send') END = 'auto' THEN 2 ELSE 1 END,
       CASE WHEN CASE WHEN json_valid(wa.permissions_json) THEN json_extract(wa.permissions_json, '$.send') END = 'auto' THEN 2 ELSE 1 END,
       m.user_id, CAST(strftime('%s', 'now') AS INTEGER) * 1000, CAST(strftime('%s', 'now') AS INTEGER) * 1000
FROM memberships m LEFT JOIN workspace_agents wa ON wa.workspace_id = m.workspace_id;

UPDATE cases SET agent_id = (SELECT a.id FROM agents a WHERE a.workspace_id = cases.workspace_id AND a.kind = 'lead');
UPDATE automations SET agent_id = (SELECT a.id FROM agents a WHERE a.workspace_id = automations.workspace_id AND a.kind = 'lead');
INSERT INTO automation_versions (automation_id, version, definition_json, created_by, created_at)
SELECT id, 1, json_object('name', name, 'enabled', enabled, 'trigger', "trigger",
                          'conditions', CASE WHEN json_valid(conditions_json) THEN json(conditions_json) ELSE json('[]') END, 'actions', CASE WHEN json_valid(actions_json) THEN json(actions_json) ELSE json('[]') END), NULL, created_at
FROM automations;
