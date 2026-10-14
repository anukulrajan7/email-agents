export type Role = 'admin' | 'operator';
export type SourceKind = 'forwarding' | 'nango_gmail' | 'nango_microsoft' | 'future';
export type SourceStatus = 'pending' | 'connected' | 'degraded' | 'disconnected';
export type ProcessingStatus =
  | 'pending'
  | 'normalized'
  | 'classified'
  | 'workflow_running'
  | 'workflow_completed'
  | 'processed'
  | 'failed';
export type Category = string;
export type Priority = 'low' | 'normal' | 'high';
export type ClassSource = 'jev' | 'llm' | 'manual' | 'fallback';
export type DraftStatus = 'suggested' | 'edited' | 'approved' | 'scheduled' | 'sent';
export type ReviewStatus = 'open' | 'closed';
export type RunStatus = 'running' | 'completed' | 'failed' | 'skipped';
export type JobStatus = 'pending' | 'running' | 'succeeded' | 'dead';

export type ErrorCode =
  | 'unauthorized'
  | 'forbidden'
  | 'not_found'
  | 'bad_request'
  | 'conflict'
  | 'rate_limited'
  | 'email_not_verified'
  | 'email_not_sent'
  | 'send_blocked'
  | 'not_implemented'
  | 'internal';

export type ApiErrorBody = { error: { code: string; message: string; request_id: string } };

export type LabelColor = 'slate' | 'sage' | 'plum' | 'ochre' | 'teal' | 'postmark' | 'ink' | 'rule';
export type Label = {
  id: string;
  key: string;
  name: string;
  description: string;
  color: LabelColor;
  position: number;
  system: boolean;
  archived: boolean;
  message_count: number;
};
export type LabelTemplate = 'hr' | 'general' | 'blank';
export type LabelTemplateInfo = {
  key: LabelTemplate;
  name: string;
  description: string;
  labels: { key: string; name: string; description: string }[];
};
export type LabelCreateBody = { name: string; description: string; color?: LabelColor };
export type LabelPatch = Partial<{ name: string; description: string; color: LabelColor; archived: boolean; position: number }>;

export type Workspace = { id: string; name: string; role: Role; created_at: string; packs?: string[]; goal?: string | null };

export type Pack = {
  key: string;
  name: string;
  description: string;
  labels: { key: string; name: string; description: string }[];
  presets: string[];
  features: string[];
};
export type WorkspacePack = Pack & { enabled: boolean };
export type PackLabelRef = { key: string; name: string };
export type PackLabelChanges = { labels_added: PackLabelRef[]; labels_restored: PackLabelRef[]; labels_hidden: PackLabelRef[] };
export type WorkspacePacks = { enabled: string[]; items: WorkspacePack[] } & Partial<PackLabelChanges>;

export type MeResponse = {
  user: { id: string; email: string; name: string | null };
  workspaces: Workspace[];
  auth_mode: 'dev' | 'oidc' | 'password';
};

export type AuthConfig = { mode: 'password' | 'dev' | 'oidc'; signup_enabled: boolean; email_verification?: boolean };
export type AuthSession = {
  token: string;
  expires_at: string;
  user: { id: string; email: string; name: string | null };
};
export type SignupBody = { email: string; password: string; name?: string };
export type VerificationRequired = { verification_required: true; email: string };
export type SignupResponse = AuthSession | VerificationRequired;
export type VerifyBody = { email: string; code: string };
export type LoginBody = { email: string; password: string };

export type Source = {
  id: string;
  kind: SourceKind;
  status: SourceStatus;
  inbound_address: string | null;
  capabilities: {
    read: boolean;
    sync: boolean;
    provider_labels: boolean;
    provider_drafts: boolean;
    send: boolean;
  };
  last_synced_at: string | null;
  last_error: string | null;
  created_at: string;
  /** Set once the mailbox was disconnected with its mail deleted; `done` when nothing is left to delete. */
  purge?: { status: 'running' | 'done' | 'failed'; total: number; done: number; error?: string | null } | null;
};

/** What disconnecting a mailbox will permanently delete from Post Room. */
export type PurgePreview = { emails: number; attachments: number; drafts: number; summaries: number; scheduled_replies: number; open_tickets: number };

/** What a delete will cancel besides the emails themselves. */
export type DeletePreview = { emails: number; scheduled_replies: number };

export type DeleteResult = { deleted: number };

export type NangoProvider = 'gmail' | 'microsoft';

export type NangoSession = { token: string; expires_at: string; provider_config_key: string };

export type NangoConnectionBody = { connection_id: string; provider_config_key: string };

export type MessageSummary = {
  id: string;
  subject: string;
  from: string;
  original_from: string | null;
  received_at: string;
  processing_status: ProcessingStatus;
  category: Category | null;
  priority: Priority | null;
  needs_review: boolean;
  attachment_count: number;
  needs_reply?: boolean;
  summary?: string | null;
  unread?: boolean;
  /** We already replied (sent from here, or answered in Gmail in the same thread). */
  replied?: boolean;
};

export type Classification = {
  category: Category;
  priority: Priority;
  requires_reply: boolean;
  confidence: number | null;
  needs_review: boolean;
  source: ClassSource;
  model_id: string | null;
  updated_at: string;
};

export type MemoryScope = 'workspace' | 'contact';
export type MemoryKind = 'fact' | 'preference' | 'style';
export type MemoryStatus = 'active' | 'suggested' | 'archived';
export type MemorySource = 'user' | 'learned';

export type Memory = {
  id: string;
  scope: MemoryScope;
  contact_email: string | null;
  kind: MemoryKind;
  content: string;
  status: MemoryStatus;
  source: MemorySource;
  created_by: string | null;
  evidence_message_id: string | null;
  created_at: string;
  updated_at: string;
};

export type Contact = {
  email: string;
  display_name: string | null;
  message_count: number;
  first_seen_at: string;
  last_seen_at: string;
  top_categories: { category: Category; count: number }[];
};

export type Draft = {
  id: string;
  message_id: string;
  status: DraftStatus;
  body_text: string;
  model_id: string | null;
  memories_used: { id: string; content: string }[];
  unanswered?: string[];
  created_at: string;
  updated_at: string;
  /** While status is 'scheduled': when the reply goes out (undo until then). */
  send_at?: string | null;
  /** Why the last scheduled send didn't go out. */
  send_error?: string | null;
};

export type SafetyIssue = { code: string; message: string; fix: string | null };
export type SafetyReport = { blocking: SafetyIssue[]; warnings: SafetyIssue[] };
export type ScheduledSend = { status: 'scheduled'; draft_id: string; send_at: string };

export type ExtractKind = 'sheet' | 'office' | 'pdf' | 'docx' | 'image' | 'audio' | 'unsupported';
export type ExtractStatus = 'done' | 'failed' | 'skipped';

/** Small facts the reader found, for the expanded attachment view. Fields depend on the kind. */
export type ExtractMeta = {
  summary?: string;
  /** sheet: a few rows per sheet (first row is the header). */
  sheets?: { name: string; rows: number; columns: number; truncated: boolean; sample: string[][] }[];
  /** image */
  image_kind?: string;
  text?: string;
  fields?: Record<string, string>;
  /** audio */
  action_items?: string[];
  transcript_excerpt?: string;
  truncated?: boolean;
};

export type Attachment = {
  id: string;
  name: string;
  media_type: string | null;
  size_bytes: number | null;
  scan_status: string;
  /** Absent or null until the attachment has been looked at. */
  extract_kind?: ExtractKind | null;
  extract_status?: ExtractStatus | null;
  /** Why a file was skipped or failed, in plain words. */
  extract_note?: string | null;
  /** Up to 300 characters: summary, description or the first words. */
  preview?: string | null;
  extract_meta?: ExtractMeta | null;
};

export type AuditEvent = {
  id: string;
  actor_type: 'user' | 'system';
  actor_id: string | null;
  action: string;
  entity_type: string;
  entity_id: string;
  created_at: string;
};

export type Insight = {
  summary: string;
  asks: string[];
  deadline: string | null;
  needs_reply: boolean;
  missing_info: string[];
};

export type MessageDetail = {
  id: string;
  source: { id: string; kind: SourceKind; can_create_provider_draft: boolean; can_send: boolean };
  from: string;
  original_from: string | null;
  to: string[];
  cc?: string[];
  subject: string;
  body_text: string;
  body_html_safe: string | null;
  received_at: string;
  processing_status: ProcessingStatus;
  attachments: Attachment[];
  classification: Classification | null;
  draft: Draft | null;
  review: { id: string; status: ReviewStatus; kind: string } | null;
  workflow_run: { id: string; status: RunStatus; error: string | null } | null;
  audit: AuditEvent[];
  contact: Contact | null;
  contact_memories: Memory[];
  insight?: Insight | null;
  replied?: boolean;
  timesheet_records?: TimesheetChip[];
};

/** A timesheet recorded from an email (the full record is in the HRMS Timesheets page). */
export type TimesheetChip = {
  id: string;
  status: 'recorded' | 'replaced' | 'needs_check';
  period_start: string;
  period_end: string;
  total_hours: number | null;
};

export type ReviewItem = {
  id: string;
  kind: string;
  title: string;
  status: ReviewStatus;
  message_id: string;
  message_subject: string;
  assigned_user_id: string | null;
  created_at: string;
};

export type Workflow = {
  id: string;
  template_key: 'timesheet_review' | 'leave_request' | 'candidate_referral';
  label_key: string;
  name: string;
  description: string;
  enabled: boolean;
  available: boolean;
  last_run: { status: RunStatus; created_at: string } | null;
};

export type WorkflowRun = {
  id: string;
  workflow_id: string;
  template_key: string;
  message_id: string;
  message_subject: string;
  status: RunStatus;
  error: string | null;
  created_at: string;
};

export type Job = {
  id: string;
  kind: string;
  status: JobStatus;
  attempts: number;
  max_attempts: number;
  last_error: string | null;
  run_at: string;
  updated_at: string;
};

export type Paged<T> = { items: T[]; next_cursor: string | null };
export type Items<T> = { items: T[] };

export type ClassificationPatch = {
  category: Category;
  priority?: Priority;
  requires_reply?: boolean;
};
export type DraftPatch = { body_text?: string; status?: 'edited' | 'approved' };
export type TestEmailBody = { from?: string; subject?: string; body_text?: string };

export type MemoryCreateBody = {
  scope: MemoryScope;
  contact_email?: string;
  kind: MemoryKind;
  content: string;
};
export type MemoryPatch = { content?: string; kind?: MemoryKind; status?: MemoryStatus };
export type ContactDetail = { contact: Contact; memories: Memory[]; recent_messages: MessageSummary[] };

export type MatchSource = 'llm' | 'rules' | 'manual';
export type MatchStatus = 'suggested' | 'shortlisted' | 'dismissed';
export type Requirement = {
  id: string;
  message_id: string;
  client: string | null;
  role_title: string;
  must_have_skills: string[];
  nice_to_have_skills: string[];
  min_experience: number | null;
  max_experience: number | null;
  location: string | null;
  positions: number | null;
  status: 'open' | 'closed';
  source: MatchSource;
  created_at: string;
  match_count: number;
  top_score: number | null;
};
export type Candidate = {
  id: string;
  message_id: string;
  name: string;
  email: string | null;
  skills: string[];
  total_experience: number | null;
  location: string | null;
  notice_days: number | null;
  referred_by: string | null;
  summary: string | null;
  status: 'active' | 'placed' | 'archived';
  source: MatchSource;
  created_at: string;
  match_count: number;
};
export type Match = {
  id: string;
  score: number;
  status: MatchStatus;
  reasons: { ok: boolean; text: string }[];
  requirement: { id: string; role_title: string; client: string | null; location: string | null };
  candidate: { id: string; name: string; total_experience: number | null; location: string | null; notice_days: number | null };
  created_at: string;
  updated_at: string;
};

// ---------------------------------------------------------------- agent registry (WP-B)

export type Autonomy = 0 | 1 | 2 | 3;
export type AgentKind = 'lead' | 'system' | 'custom';
export type AgentScope = 'workspace' | 'personal';
export type AgentStatus = 'draft' | 'active' | 'paused' | 'archived';
export type AgentScopeName = 'read_mail' | 'draft' | 'send' | 'label' | 'schedule';
export type AgentCategory = 'general' | 'support' | 'sales' | 'finance' | 'personal' | 'hrms' | 'custom';

export type SpecMatch = {
  labels: string[];
  from_domains: string[];
  from_addresses: string[];
  subject_keywords: string[];
  needs_reply: boolean | null;
  priority: 'low' | 'normal' | 'high' | null;
  attachment_types: string[];
};
export type SpecTrigger =
  | { type: 'email_received'; match: SpecMatch }
  | { type: 'schedule'; every_hours: number }
  | { type: 'manual' };
export type SpecStep =
  | { type: 'extract'; fields: { name: string; type: 'string' | 'number' | 'date' | 'boolean'; description: string }[] }
  | { type: 'draft_reply'; quality: 'fast' | 'balanced' | 'best' }
  | { type: 'create_review'; title: string }
  | { type: 'remind_if_no_reply'; hours: number }
  | { type: 'save_to_gmail_drafts' }
  | { type: 'label'; value: string }
  | { type: 'start_case' }
  | { type: 'notify'; to: 'owner' | 'team' };
export type AgentSpec = {
  name: string;
  key: string;
  category: AgentCategory;
  description: string;
  goal: string;
  mode: 'workflow' | 'agent';
  triggers: SpecTrigger[];
  steps: SpecStep[];
  tools: string[];
  scopes: AgentScopeName[];
  permissions: { draft: boolean; send: 'never' | 'ask' | 'earned'; max_autonomy: Autonomy; daily_send_cap: number };
  memory: { scope: 'own+shared' | 'own' | 'shared'; learn_from: ('edits' | 'approvals' | 'labels' | 'routes')[] };
  drafting: { rules: string[]; never_do: string[]; quality_tier: 'fast' | 'balanced' | 'best' };
  budget: { max_steps: number; usd_per_case: number };
  questions_for_user: string[];
  assumptions: string[];
};
export type SpecIssue = { path: string; code: string; message: string };

export type AgentSummary = {
  id: string;
  key: string;
  kind: AgentKind;
  scope: AgentScope;
  owner_user_id: string | null;
  category: AgentCategory;
  name: string;
  avatar: string;
  color: string;
  description: string;
  status: AgentStatus;
  paused_reason: string | null;
  template_key: string | null;
  tuned: boolean;
  update_available: boolean;
  hidden?: boolean;
  active_version: number | null;
  autonomy: { earned: Autonomy; cap: Autonomy; effective: Autonomy; limited_by: 'earned' | 'admin_cap' | 'spec' | 'workspace' | 'profile' | 'paused' };
  today: { routed: number; cases: number; approvals_pending: number; cost_usd: number };
};
export type AgentVersionInfo = {
  version: number;
  source: 'template' | 'tuned' | 'designer' | 'manual' | 'rollback' | 'backfill';
  gate_status: 'pending' | 'passed' | 'failed' | 'overridden';
  note: string | null;
  spec_hash: string;
  active: boolean;
  created_at: string;
};
export type AgentDetail = AgentSummary & { spec: AgentSpec; versions: AgentVersionInfo[] };
export type AgentList = {
  paused_all: boolean;
  me: AgentSummary | null;
  categories: { key: AgentCategory; name: string; agents: AgentSummary[] }[];
};
export type AgentTemplateInfo = {
  key: string;
  category: AgentCategory;
  pack: string;
  version: number;
  scope: AgentScope;
  name: string;
  avatar: string;
  description: string;
  mode: 'workflow' | 'agent';
  max_autonomy: Autonomy;
  installed: boolean;
  pack_enabled: boolean;
  spec: AgentSpec;
};
export type VersionDiffEntry = { path: string; before: unknown; after: unknown };
export type MessageRoute = {
  agent_id: string;
  agent_name: string;
  agent_version: number;
  source: 'manual' | 'thread' | 'rule' | 'label' | 'personal' | 'generic' | 'fallback';
  shadow: boolean;
  created_at: string;
};
export type OutgoingItem = { draft_id: string; message_id: string; subject: string; to: string; send_at: string; body_preview: string };
export type AgentAuditItem = {
  at: string;
  case_id: string;
  message_id: string | null;
  tool: string;
  scope: AgentScopeName;
  status: 'ok' | 'error' | 'blocked' | 'waiting_approval' | 'simulated';
  turn: number;
  shadow: boolean;
  refs: { message_id: string | null; draft_id: string | null; requirement_id: string | null; candidate_ids: string[] | null };
};

export type AgentPatch = Partial<{ status: 'active' | 'paused' | 'archived'; name: string; avatar: string; color: string; autonomy_cap: Autonomy }>;
