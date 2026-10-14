export type SpecIssue = { code: string; path?: string; message: string };

export type SpecStep =
  | { type: 'extract'; fields: { name: string; type: string; description: string }[] }
  | { type: 'draft_reply'; quality: string }
  | { type: 'create_review'; title: string }
  | { type: 'remind_if_no_reply'; hours: number }
  | { type: 'save_to_gmail_drafts' }
  | { type: 'label'; value: string }
  | { type: 'start_case' }
  | { type: 'notify'; to: string };

export type SpecTrigger =
  | {
      type: 'email_received';
      match: {
        labels: string[];
        from_domains: string[];
        from_addresses: string[];
        subject_keywords: string[];
        needs_reply: boolean | null;
        priority: string | null;
      };
    }
  | { type: 'schedule'; every_hours: number }
  | { type: 'manual' };

export type DesignedSpec = {
  name: string;
  key: string;
  description: string;
  goal: string;
  mode: 'workflow' | 'agent';
  triggers: SpecTrigger[];
  steps: SpecStep[];
  permissions: { draft: boolean; send: 'never' | 'ask' | 'earned'; max_autonomy: number };
  drafting: { rules: string[]; never_do: string[] };
};

export type DryRunExample = {
  message_id: string;
  subject: string;
  route_reason: string;
  currently_handled_by: string | null;
  steps: { summary: string; status: string }[];
  draft: string | null;
  needs_person: boolean;
};

export type DryRunReport = {
  status: 'running' | 'done' | 'failed';
  error?: string;
  would_handle?: number;
  sampled?: number;
  steals?: { agent_id: string; name: string; count: number }[];
  examples?: DryRunExample[];
  est_cost_per_email_usd?: number;
  est_monthly_usd?: number;
  warnings?: string[];
};

export type DesignerSession = {
  id: string;
  goal: string;
  agent_id: string | null;
  base_version: number | null;
  status: 'drafting' | 'ready' | 'saved' | 'abandoned';
  turns: { role: 'user' | 'designer'; text: string }[];
  spec: DesignedSpec | null;
  issues: SpecIssue[];
  questions: string[];
  assumptions: string[];
  dry_run: DryRunReport | null;
  cost_usd: number;
  limits: { calls_today: number; daily_limit: number; session_budget_usd: number };
};

export type SavedAgent = { agent: { id: string; name: string }; version: number; session: DesignerSession };
