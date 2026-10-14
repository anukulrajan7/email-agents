-- Messaging: Meera (and the agents) can send a new email, an in-app message or a notice to the owner,
-- ask someone and report the answer back, and run a support case with a vendor.
-- Nothing is rebuilt except agent_approvals (its allowed actions widen by one).

-- Teams: a name and a list of members. Members are workspace members; a team can name a desk agent.
CREATE TABLE teams (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  name TEXT NOT NULL,
  desk_agent_id TEXT,
  created_by TEXT,
  created_at INTEGER NOT NULL
);
CREATE UNIQUE INDEX uq_teams_name ON teams(workspace_id, lower(name));

CREATE TABLE team_members (
  team_id TEXT NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  PRIMARY KEY (team_id, user_id)
);

-- In-app messages and notices. Private to the recipient (and, for what they sent, the sender):
-- every read is by recipient_user_id or sender_user_id. A held message is not visible to the recipient
-- before deliver_at, and never after the sender stops it (cancelled_at).
CREATE TABLE in_app_messages (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  thread_key TEXT NOT NULL,
  sender_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
  sender_agent_id TEXT,
  recipient_user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  team_id TEXT,
  -- message | notice
  kind TEXT NOT NULL CHECK (kind IN ('message','notice')),
  subject TEXT NOT NULL,
  body TEXT NOT NULL,
  link_json TEXT NOT NULL DEFAULT 'null',
  -- assistant | agent | person | system
  via TEXT NOT NULL,
  deliver_at INTEGER NOT NULL,
  cancelled_at INTEGER,
  -- 1 while an automatic message waits for its due-time re-check; the recipient sees it only at 0.
  needs_check INTEGER NOT NULL DEFAULT 0,
  read_at INTEGER,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_inapp_recipient ON in_app_messages(workspace_id, recipient_user_id, deliver_at DESC);
CREATE INDEX idx_inapp_sender ON in_app_messages(workspace_id, sender_user_id, created_at DESC);
CREATE INDEX idx_inapp_thread ON in_app_messages(workspace_id, thread_key);

-- A new email Meera or an agent sends from a person's own mailbox (a reply to an email is a draft; this is not).
CREATE TABLE outbound_emails (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  source_id TEXT NOT NULL REFERENCES email_sources(id),
  owner_user_id TEXT REFERENCES users(id) ON DELETE SET NULL,
  agent_id TEXT,
  to_address TEXT NOT NULL,
  to_label TEXT NOT NULL DEFAULT '',
  subject TEXT NOT NULL,
  body_text TEXT NOT NULL,
  -- member | directory | known_external | new_external
  recipient_kind TEXT NOT NULL,
  -- agent | person
  authored_by TEXT NOT NULL CHECK (authored_by IN ('agent','person')),
  -- Why a person has to approve it (set while awaiting approval).
  approval_reason TEXT,
  status TEXT NOT NULL CHECK (status IN ('awaiting_approval','scheduled','sent','cancelled','blocked')),
  auto_sent INTEGER NOT NULL DEFAULT 0,
  send_at INTEGER,
  send_requested_by TEXT,
  send_error TEXT,
  send_attempt_id TEXT,
  send_started_at INTEGER,
  provider_message_id TEXT,
  provider_thread_id TEXT,
  -- Our Message-ID for the send, and the Message-ID a follow-up answers.
  rfc_message_id TEXT,
  in_reply_to TEXT,
  watch_id TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_outbound_ws ON outbound_emails(workspace_id, owner_user_id, created_at DESC);
CREATE INDEX idx_outbound_due ON outbound_emails(send_at) WHERE send_at IS NOT NULL;

-- Something Meera asked on a person's behalf and is waiting on. A reply (an email in the thread, an in-app
-- reply, or a finished desk case) wakes it; one nudge goes out after nudge_after_at if nobody answered.
CREATE TABLE watches (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  agent_id TEXT,
  -- ask | ticket | desk
  kind TEXT NOT NULL CHECK (kind IN ('ask','ticket','desk')),
  title TEXT NOT NULL,
  -- email | in_app | desk
  channel TEXT NOT NULL CHECK (channel IN ('email','in_app','desk')),
  counterpart TEXT NOT NULL,
  -- waiting | replied | nudged | closed
  status TEXT NOT NULL CHECK (status IN ('waiting','replied','nudged','closed')),
  outbound_email_id TEXT,
  thread_key TEXT,
  source_id TEXT,
  provider_thread_id TEXT,
  case_id TEXT,
  question TEXT NOT NULL DEFAULT '',
  nudge_hours INTEGER NOT NULL DEFAULT 24,
  nudge_after_at INTEGER,
  nudged_at INTEGER,
  answer_text TEXT,
  answer_message_id TEXT,
  answered_at INTEGER,
  last_update_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_watches_user ON watches(workspace_id, user_id, last_update_at DESC);
CREATE INDEX idx_watches_thread ON watches(source_id, provider_thread_id) WHERE provider_thread_id IS NOT NULL;
CREATE INDEX idx_watches_key ON watches(workspace_id, thread_key) WHERE thread_key IS NOT NULL;
CREATE INDEX idx_watches_case ON watches(case_id) WHERE case_id IS NOT NULL;

-- A vendor or bank support case Meera runs for a person (a failed card charge, a stuck SIP).
CREATE TABLE support_tickets (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  watch_id TEXT NOT NULL,
  company TEXT NOT NULL,
  support_address TEXT NOT NULL,
  -- Where the address came from: the email, a known contact, or the person.
  address_source TEXT NOT NULL,
  subject TEXT NOT NULL,
  source_message_id TEXT,
  -- open (needs a look) | waiting (we wrote, they have not answered) | resolved
  status TEXT NOT NULL CHECK (status IN ('open','waiting','resolved')),
  last_summary TEXT NOT NULL DEFAULT '',
  last_update_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_tickets_user ON support_tickets(workspace_id, user_id, last_update_at DESC);

-- Links under an assistant answer (the sent email, the in-app message).
ALTER TABLE assistant_messages ADD COLUMN links_json TEXT NOT NULL DEFAULT '[]';

-- send_message joins the actions a person can approve. SQLite can't change a CHECK in place, so the table is rebuilt.
CREATE TABLE agent_approvals_new (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
  case_id TEXT NOT NULL REFERENCES cases(id) ON DELETE CASCADE,
  step_seq INTEGER,
  action TEXT NOT NULL CHECK (action IN ('send_email','submit_candidate','create_automation','send_message')),
  summary TEXT NOT NULL,
  to_address TEXT,
  subject TEXT,
  body TEXT NOT NULL DEFAULT '',
  draft_id TEXT REFERENCES drafts(id) ON DELETE SET NULL,
  requirement_id TEXT REFERENCES requirements(id) ON DELETE SET NULL,
  candidates_json TEXT NOT NULL DEFAULT '[]',
  input_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','approved','declined')),
  decided_by TEXT REFERENCES users(id),
  decided_at INTEGER,
  note TEXT,
  created_at INTEGER NOT NULL,
  agent_id TEXT,
  approved_hash TEXT
);
INSERT INTO agent_approvals_new (id, workspace_id, case_id, step_seq, action, summary, to_address, subject, body, draft_id,
                                 requirement_id, candidates_json, input_json, status, decided_by, decided_at, note, created_at,
                                 agent_id, approved_hash)
  SELECT id, workspace_id, case_id, step_seq, action, summary, to_address, subject, body, draft_id,
         requirement_id, candidates_json, input_json, status, decided_by, decided_at, note, created_at,
         agent_id, approved_hash
  FROM agent_approvals;
DROP TABLE agent_approvals;
ALTER TABLE agent_approvals_new RENAME TO agent_approvals;
CREATE INDEX idx_approvals_ws ON agent_approvals(workspace_id, status, created_at DESC);
CREATE INDEX idx_approvals_case ON agent_approvals(case_id, created_at);
CREATE UNIQUE INDEX uq_approvals_pending_case ON agent_approvals(case_id) WHERE status = 'pending';
