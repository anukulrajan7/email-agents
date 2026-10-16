-- S8 round 3: the scans that group unmatched sheets and list weak links read only a few rows of a large table.
CREATE INDEX idx_timesheet_records_ungrouped ON timesheet_records(workspace_id) WHERE person_id IS NULL AND identity_id IS NULL AND status != 'replaced';
CREATE INDEX idx_timesheet_records_weak_link ON timesheet_records(workspace_id, period_start DESC) WHERE match_method = 'name' AND match_confidence = 0.7;
