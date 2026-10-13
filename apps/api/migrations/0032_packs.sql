-- Packs: a workspace turns on use-case bundles (labels, presets, reply style, features).
-- HR/recruiting is the optional `staffing` pack; every workspace has `general`.

CREATE TABLE workspace_packs (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  pack_key TEXT NOT NULL,
  enabled_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, pack_key)
);

-- What the person wanted the workspace for, in their own words (goal-first onboarding).
ALTER TABLE workspaces ADD COLUMN goal TEXT;

-- Every workspace that exists today keeps today's behaviour: all of them get the staffing pack.
INSERT OR IGNORE INTO workspace_packs (workspace_id, pack_key, enabled_at)
SELECT w.id, 'staffing', CAST(strftime('%s', 'now') AS INTEGER) * 1000 FROM workspaces w;

INSERT OR IGNORE INTO workspace_packs (workspace_id, pack_key, enabled_at)
SELECT w.id, 'general', CAST(strftime('%s', 'now') AS INTEGER) * 1000 FROM workspaces w;
