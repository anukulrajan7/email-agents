-- New mail must never wait behind bulk work: jobs are claimed by priority, then age.
ALTER TABLE jobs ADD COLUMN priority INTEGER NOT NULL DEFAULT 0;
CREATE INDEX idx_claim_jobs_priority ON jobs(status, priority DESC, run_at);
