import type { Priority } from '../../api/types';

export type Trigger = 'email_received';

export type Condition =
  | { field: 'label'; op: 'is' | 'is_not'; value: string }
  | { field: 'priority'; op: 'is'; value: Priority }
  | { field: 'needs_reply'; op: 'is'; value: boolean }
  | { field: 'from'; op: 'contains'; value: string }
  | { field: 'subject'; op: 'contains'; value: string };

export type Action =
  | { type: 'draft_reply'; instructions?: string }
  | { type: 'save_to_gmail_drafts' }
  | { type: 'send_reply'; daily_cap: number; allowed_domains: string[] }
  | { type: 'create_review'; title?: string }
  | { type: 'remind_if_no_reply'; hours: number };

export type ConditionField = Condition['field'];
export type ActionType = Action['type'];

export type AutomationInput = {
  name: string;
  enabled: boolean;
  conditions: Condition[];
  actions: Action[];
};

export type Automation = AutomationInput & {
  id: string;
  trigger: Trigger;
  preset_key: string | null;
  run_count: number;
  last_run_at: string | null;
  created_at: string;
  updated_at: string;
};

export type Preset = { key: string; name: string; description: string; automation: AutomationInput };

export type AutomationRunStatus = 'completed' | 'skipped' | 'failed';
export type AutomationActionStatus = 'done' | 'skipped' | 'failed';

export type AutomationRun = {
  id: string;
  automation_id: string;
  message_id: string;
  message_subject: string;
  status: AutomationRunStatus;
  actions: { type: string; status: AutomationActionStatus; detail: string | null }[];
  created_at: string;
};

export type TestResult = { matched: { message_id: string; subject: string }[] };
