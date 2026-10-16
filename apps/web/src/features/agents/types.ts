import type { Action, Condition } from '../automations/types';

export type CaseStatus = 'running' | 'waiting_approval' | 'waiting_event' | 'done' | 'failed' | 'cancelled';
export type CaseKind = 'requirement' | 'inbox';

export interface CaseSummary {
  id: string;
  kind: CaseKind;
  title: string;
  goal: string;
  status: CaseStatus;
  outcome: string | null;
  steps: number;
  max_steps: number;
  spent_usd: number;
  budget_usd: number;
  message_id: string | null;
  requirement_id: string | null;
  next_wake_at: string | null;
  created_at: string;
  updated_at: string;
  agent?: AgentBadge | null;
  working_for?: WorkingFor | null;
}

export interface Step {
  seq: number;
  kind: 'tool' | 'critic' | 'approval' | 'note' | 'finish';
  tool: string | null;
  summary: string;
  input: unknown;
  output: unknown;
  status: 'ok' | 'error' | 'blocked' | 'waiting';
  cost_usd: number;
  created_at: string;
}

export interface Approval {
  id: string;
  case_id: string;
  case_title: string;
  action: 'send_email' | 'submit_candidate' | 'create_automation';
  summary: string;
  to: string | null;
  subject: string | null;
  body: string;
  candidates: { id: string; name: string; score: number }[];
  status: 'pending' | 'approved' | 'declined';
  decided_by: string | null;
  decided_at: string | null;
  created_at: string;
  agent?: AgentBadge | null;
  working_for?: WorkingFor | null;
  automation?: SuggestedAutomation | null;
  /** What the draft could not confirm (it says it will confirm later). */
  unanswered?: string[];
  /** Who asked Meera to write this reply, in one line ('Rohan asked Meera to ...'). */
  handoff?: { line: string; newer?: string; from: { agent_id: string; name: string; kind: string; job: string }[] } | null;
}

export type AgentColor = 'slate' | 'sage' | 'plum' | 'ochre' | 'teal' | 'postmark' | 'ink';
export type AgentBadge = {
  name: string;
  avatar: string;
  color: AgentColor | string;
  kind?: string;
  scope?: string;
  category?: string;
  template_key?: string | null;
};
export type WorkingFor = { user_id: string; email: string; name: string | null };
export type SuggestedAutomation = { name: string; conditions: Condition[]; actions: Action[] };

export interface CaseDetail extends Omit<CaseSummary, 'steps'> {
  state: Record<string, unknown>;
  steps: Step[];
  approvals: Approval[];
}

export type CaseList = { items: CaseSummary[] };
export type ApprovalList = { items: Approval[]; pending_count: number };

export type StartCaseBody = { kind: 'requirement'; requirement_id: string } | { kind: 'inbox'; message_id: string };
export type ApprovalDecision = { decision: 'approve' | 'decline'; edited_body?: string; note?: string };

export type CaseFilter = 'all' | 'working' | 'waiting' | 'done' | 'failed';

export type AgentTask = 'reply' | 'requirements';
export type SendMode = 'never' | 'ask' | 'auto';
export type AgentQuality = 'fast' | 'balanced' | 'best';

export interface WorkspaceAgent extends AgentBadge {
  persona: string;
  signature: string | null;
  tasks: AgentTask[];
  permissions: { draft: boolean; send: SendMode; submit_candidate: 'ask' };
  quality: AgentQuality;
  enabled: boolean;
  updated_at: string;
}

export type AgentView = { agent: WorkspaceAgent; standard_guidance: string; can_edit: boolean };

export type AgentInput = Partial<{
  name: string;
  avatar: string;
  color: string;
  persona: string;
  signature: string | null;
  tasks: AgentTask[];
  permissions: { draft?: boolean; send?: SendMode };
  quality: AgentQuality;
  enabled: boolean;
}>;

export type Handle = 'client_requirements' | 'candidates' | 'timesheets' | 'leave' | 'payroll' | 'onboarding' | 'general';
export type Audience = 'clients' | 'candidates' | 'colleagues' | 'vendors';
export type Formality = '' | 'casual' | 'friendly' | 'formal';
export type ReplyLength = '' | 'short' | 'medium' | 'detailed';
export type SendApproval = 'always_ask' | 'routine_ok';

export interface UserProfile {
  display_name: string;
  job_title: string;
  handles: Handle[];
  handles_note: string;
  ignore_note: string;
  audiences: Audience[];
  vips: string[];
  tone: { formality: Formality; length: ReplyLength; sign_off: string | null; language: string };
  working_hours: string;
  timezone: string;
  never_do: string;
  approvals: { send_email: SendApproval; submit_candidate: 'always_ask' };
  saved: boolean;
  updated_at: string | null;
}

export type UserProfileInput = Omit<UserProfile, 'saved' | 'updated_at' | 'approvals'> & { approvals: { send_email: SendApproval } };

export type MyProfileView = { profile: UserProfile; agent: AgentBadge; mailboxes: string[]; needs_onboarding: boolean };

export type TeamProfile = { user_id: string; email: string; role: 'admin' | 'operator'; mailboxes: string[]; profile: UserProfile | null };

export type PreviewBody = { message_id: string; agent?: AgentInput; profile?: Partial<UserProfileInput> };
export type PreviewResult = { body: string; needs_reply: boolean; agent: AgentBadge };
