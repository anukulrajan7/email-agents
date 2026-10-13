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
  action: 'send_email' | 'submit_candidate';
  summary: string;
  to: string | null;
  subject: string | null;
  body: string;
  candidates: { id: string; name: string; score: number }[];
  status: 'pending' | 'approved' | 'declined';
  decided_by: string | null;
  decided_at: string | null;
  created_at: string;
}

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
