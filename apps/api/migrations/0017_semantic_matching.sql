-- Semantic matching (M4): one profile embedding per requirement / candidate for vector recall, and a
-- once-per-day claim for the match digest email.

-- content_hash = sha256(embedding model + profile text): a record is re-embedded only when its profile changes.
-- `embedding` (little-endian f32, 1024-d, L2-normalised) is kept here for pairwise similarity; the vec0 table
-- shares the rowid and serves KNN recall within a workspace.
CREATE TABLE match_profiles (
  id INTEGER PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  record_kind TEXT NOT NULL CHECK (record_kind IN ('requirement','candidate')),
  record_id TEXT NOT NULL,
  content_hash TEXT NOT NULL,
  embedding BLOB NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (record_kind, record_id)
);
CREATE INDEX idx_match_profiles_ws ON match_profiles(workspace_id, record_kind);

CREATE VIRTUAL TABLE match_profile_vec USING vec0(
  workspace_id TEXT PARTITION KEY,
  record_kind TEXT,
  embedding FLOAT[1024] distance_metric=cosine
);

CREATE TRIGGER match_profile_vec_ad AFTER DELETE ON match_profiles BEGIN
  DELETE FROM match_profile_vec WHERE rowid = old.id;
END;
CREATE TRIGGER requirements_profile_ad AFTER DELETE ON requirements BEGIN
  DELETE FROM match_profiles WHERE record_kind = 'requirement' AND record_id = old.id;
END;
CREATE TRIGGER candidates_profile_ad AFTER DELETE ON candidates BEGIN
  DELETE FROM match_profiles WHERE record_kind = 'candidate' AND record_id = old.id;
END;

-- New strong matches per day.
CREATE INDEX idx_matches_digest ON matches(workspace_id, status, created_at);

-- day = days since the Unix epoch (UTC). The row is the claim: inserted before sending, so a retried job never
-- sends the same day's digest twice.
CREATE TABLE match_digests (
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  day INTEGER NOT NULL,
  match_count INTEGER NOT NULL,
  recipients_json TEXT NOT NULL DEFAULT '[]',
  sent_at INTEGER NOT NULL,
  PRIMARY KEY (workspace_id, day)
);
