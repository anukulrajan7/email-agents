-- Referral ↔ requirement matching: attachment text, extracted records, scored matches.

ALTER TABLE attachments ADD COLUMN provider_attachment_id TEXT;
ALTER TABLE attachments ADD COLUMN text_content TEXT;
ALTER TABLE attachments ADD COLUMN text_status TEXT NOT NULL DEFAULT 'pending'
  CHECK (text_status IN ('pending','extracted','unsupported','failed'));

CREATE TABLE requirements (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  client TEXT,
  role_title TEXT NOT NULL,
  must_have_skills_json TEXT NOT NULL DEFAULT '[]',
  nice_to_have_skills_json TEXT NOT NULL DEFAULT '[]',
  min_experience REAL,
  max_experience REAL,
  location TEXT,
  positions INTEGER,
  status TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open','closed')),
  source TEXT NOT NULL CHECK (source IN ('llm','rules','manual')),
  created_at INTEGER NOT NULL,
  UNIQUE (message_id)
);
CREATE INDEX idx_requirements_ws ON requirements(workspace_id, status, created_at DESC);

CREATE TABLE candidates (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  email TEXT,
  skills_json TEXT NOT NULL DEFAULT '[]',
  total_experience REAL,
  location TEXT,
  notice_days INTEGER,
  referred_by TEXT,
  summary TEXT,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active','placed','archived')),
  source TEXT NOT NULL CHECK (source IN ('llm','rules','manual')),
  created_at INTEGER NOT NULL,
  UNIQUE (message_id)
);
CREATE INDEX idx_candidates_ws ON candidates(workspace_id, status, created_at DESC);

CREATE TABLE matches (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  requirement_id TEXT NOT NULL REFERENCES requirements(id) ON DELETE CASCADE,
  candidate_id TEXT NOT NULL REFERENCES candidates(id) ON DELETE CASCADE,
  score INTEGER NOT NULL CHECK (score BETWEEN 0 AND 100),
  reasons_json TEXT NOT NULL DEFAULT '[]',
  status TEXT NOT NULL DEFAULT 'suggested' CHECK (status IN ('suggested','shortlisted','dismissed')),
  decided_by TEXT REFERENCES users(id),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE (requirement_id, candidate_id)
);
CREATE INDEX idx_matches_ws ON matches(workspace_id, status, score DESC);
