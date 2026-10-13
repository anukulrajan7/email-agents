export type Citation = { message_id: string; subject: string };

export type PlanAction = 'draft_reply' | 'start_agent' | 'approve' | 'decline' | 'create_automation' | 'snooze_alert';

export type PlanItem = {
  id: string;
  action: PlanAction;
  summary: string;
  params: Record<string, unknown>;
};

export type CommandResponse = {
  answer: string;
  citations: Citation[];
  plan: PlanItem[];
  over_budget: boolean;
};

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
  | { status: 'error'; message: string };

export type Turn =
  | { id: number; role: 'user'; text: string }
  | { id: number; role: 'assistant'; text: string; citations: Citation[]; plan: PlanItem[]; overBudget: boolean };
