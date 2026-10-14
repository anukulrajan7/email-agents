export type Citation = { message_id: string; subject: string; attachment_id?: string; attachment_name?: string };

export type PlanAction = 'draft_reply' | 'start_agent' | 'approve' | 'decline' | 'create_automation' | 'snooze_alert';

export type PlanItem = {
  id: string;
  action: PlanAction;
  summary: string;
  params: Record<string, unknown>;
};

export type PageContext = {
  route?: string;
  message_id?: string;
  case_id?: string;
  requirement_id?: string;
};

export type CommandResponse = {
  answer: string;
  citations: Citation[];
  plan: PlanItem[];
  over_budget: boolean;
};

export type StoredPlanItem = PlanItem & { state: 'pending' | 'done' | 'expired' };

export type HistoryMessage = {
  id: number;
  role: 'user' | 'assistant';
  text: string;
  created_at: string;
  citations: Citation[];
  plan: StoredPlanItem[];
};

export type HistoryPage = { items: HistoryMessage[]; next_before: number | null };

export type ConfirmLink = {
  kind: 'message' | 'case' | 'approvals' | 'automations' | 'alerts';
  id?: string;
};

export type ConfirmResponse = {
  ok: true;
  action: PlanAction;
  message: string;
  link: ConfirmLink | null;
};

export type PlanState =
  | { status: 'pending' }
  | { status: 'confirming' }
  | { status: 'done'; message: string; link: ConfirmLink | null }
  | { status: 'cancelled' }
  | { status: 'expired' }
  | { status: 'error'; message: string };

export type Turn =
  | { id: string; role: 'user'; text: string; at: string }
  | { id: string; role: 'assistant'; text: string; at: string; citations: Citation[]; plan: PlanItem[]; overBudget: boolean; failed?: boolean };
