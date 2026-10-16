-- Slice S6: Rohan owns the timesheet job. One case per (message, job), the timesheet desk's rule settings, and the
-- marks a record needs so its manager is told once and can approve it.

-- A message can now carry one real case per job. Every case so far is the 'main' job. The old index allowed one real
-- case per message, which is a subset of the new one, so no row needs deduping before it is replaced.
ALTER TABLE cases ADD COLUMN job TEXT NOT NULL DEFAULT 'main';
DROP INDEX uq_cases_message;
CREATE UNIQUE INDEX uq_cases_message_job ON cases(workspace_id, kind, message_id, job) WHERE message_id IS NOT NULL AND shadow = 0;

-- summarized_at is the atomic claim for the manager summary: whoever sets it from NULL sends the one summary.
ALTER TABLE timesheet_records ADD COLUMN summarized_at INTEGER;
ALTER TABLE timesheet_records ADD COLUMN approved_by TEXT;
ALTER TABLE timesheet_records ADD COLUMN approved_at INTEGER;

-- The owner's timesheet rules beyond tolerance, grace days and client domains (which already exist).
ALTER TABLE team_settings ADD COLUMN timesheet_cadence TEXT NOT NULL DEFAULT 'monthly' CHECK (timesheet_cadence IN ('monthly', 'weekly', 'daily'));
ALTER TABLE team_settings ADD COLUMN max_hours_per_day REAL CHECK (max_hours_per_day IS NULL OR (max_hours_per_day > 0 AND max_hours_per_day <= 24));
ALTER TABLE team_settings ADD COLUMN require_signature INTEGER NOT NULL DEFAULT 0;
