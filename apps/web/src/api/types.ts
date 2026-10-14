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

export type Workspace = { id: string; name: string; role: Role; created_at: string; packs?: string[] };

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
};

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
