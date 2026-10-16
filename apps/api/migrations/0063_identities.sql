-- Slice S8: people nobody registered stay unresolved identities. A timesheet is always kept with the name exactly as
-- written; unknown names are grouped (by employee ID, else by normalised name and where the mail came from) so an admin
-- sees one card per person, not one row per sheet. Nothing is merged by itself: every link, add or dismissal is a row
-- in identity_changes with what it touched, so it can be undone. No users or memberships row is ever created here.
CREATE TABLE unresolved_identities (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  name_as_written TEXT,
  employee_code TEXT,
  sender_domain TEXT,
  sender_address TEXT,
  record_count INTEGER NOT NULL DEFAULT 0,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'linked', 'external', 'dismissed')),
  -- Who this might be, as evidence only: [{person_id, name, reason}]. Never acted on.
  suggestions_json TEXT NOT NULL DEFAULT '[]',
  -- The person an admin chose (status linked).
  person_id TEXT REFERENCES people(id) ON DELETE SET NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (workspace_id, key)
);
CREATE INDEX idx_unresolved_identities_status ON unresolved_identities(workspace_id, status, updated_at DESC);

CREATE TABLE identity_changes (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK (kind IN ('link', 'unlink', 'alias', 'fill_field', 'add_email', 'create_person', 'dismiss', 'mark_external', 'reopen')),
  person_id TEXT,
  identity_id TEXT,
  record_ids_json TEXT NOT NULL DEFAULT '[]',
  before_json TEXT NOT NULL DEFAULT '{}',
  after_json TEXT NOT NULL DEFAULT '{}',
  actor_class TEXT NOT NULL CHECK (actor_class IN ('user', 'agent', 'system')),
  actor_id TEXT,
  evidence_json TEXT NOT NULL DEFAULT '{}',
  -- The change that reversed this one.
  undone_by TEXT,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_identity_changes_workspace ON identity_changes(workspace_id, created_at DESC);
CREATE INDEX idx_identity_changes_identity ON identity_changes(identity_id) WHERE identity_id IS NOT NULL;

ALTER TABLE timesheet_records ADD COLUMN identity_id TEXT REFERENCES unresolved_identities(id) ON DELETE SET NULL;
CREATE INDEX idx_timesheet_records_identity ON timesheet_records(identity_id) WHERE identity_id IS NOT NULL;
