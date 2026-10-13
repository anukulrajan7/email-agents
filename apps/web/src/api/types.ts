export type Role = 'admin' | 'operator';
export type SourceKind = 'forwarding' | 'nango_gmail' | 'nango_microsoft' | 'future';
export type SourceStatus = 'pending' | 'connected' | 'degraded' | 'disconnected';
export type ProcessingStatus = 'pending' | 'normalized' | 'classified' | 'workflow_completed' | 'failed';
export type Category =
  | 'timesheet'
  | 'leave'
  | 'candidate_referral'
  | 'approval'
  | 'payroll'
  | 'general'
  | 'other';
export type Priority = 'low' | 'normal' | 'high';
export type ClassSource = 'jev' | 'llm' | 'manual' | 'fallback';
export type DraftStatus = 'suggested' | 'edited' | 'approved' | 'sent';
export type ReviewStatus = 'open' | 'closed';
export type RunStatus = 'running' | 'completed' | 'failed' | 'skipped';
export type JobStatus = 'pending' | 'running' | 'succeeded' | 'dead';

export type ErrorCode =
  | 'unauthorized'
  | 'forbidden'
  | 'not_found'
  | 'bad_request'
  | 'conflict'
  | 'not_implemented'
  | 'internal';

export type ApiErrorBody = { error: { code: string; message: string; request_id: string } };

export type Workspace = { id: string; name: string; role: Role; created_at: string };

export type MeResponse = {
  user: { id: string; email: string };
  workspaces: Workspace[];
  auth_mode: 'dev' | 'oidc';
};

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

export type Draft = {
  id: string;
  message_id: string;
  status: DraftStatus;
  body_text: string;
  model_id: string | null;
  created_at: string;
  updated_at: string;
};

export type Attachment = {
  id: string;
  name: string;
  media_type: string | null;
  size_bytes: number | null;
  scan_status: string;
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

export type MessageDetail = {
  id: string;
  source: { id: string; kind: SourceKind; can_create_provider_draft: boolean };
  from: string;
  original_from: string | null;
  to: string[];
  subject: string;
  body_text: string;
  received_at: string;
  processing_status: ProcessingStatus;
  attachments: Attachment[];
  classification: Classification | null;
  draft: Draft | null;
  review: { id: string; status: ReviewStatus; kind: string } | null;
  workflow_run: { id: string; status: RunStatus; error: string | null } | null;
  audit: AuditEvent[];
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
