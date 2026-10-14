export interface KindCount {
  kind: string;
  label: string;
  severity: 'low' | 'medium' | 'high';
  total: number;
  open: number;
}

export interface DayIncidents {
  date: string;
  counts: Record<string, number>;
  total: number;
}

export interface OpenIncident {
  id: string;
  kind: string;
  label: string;
  severity: 'low' | 'medium' | 'high';
  agent_id: string | null;
  agent_name: string | null;
  case_id: string | null;
  message_id: string | null;
  details: Record<string, unknown> | null;
  created_at: number;
}

export interface CostHealth {
  today_usd: number;
  per_email_today_usd: number;
  per_email_7d_usd: number;
  avg_daily_7d_usd: number;
  spike: boolean;
}

export interface TaskHealth {
  task: string;
  label: string;
  calls: number;
  p50_ms: number | null;
  p95_ms: number | null;
  error_rate: number;
  refusal_rate: number;
  schema_failure_rate: number;
}

export interface ShadowReport {
  compared: number;
  failed: number;
  label_agreement: number;
  needs_reply_agreement: number;
  priority_agreement: number;
  min_agreement: number;
  min_samples: number;
  ready_to_switch: boolean;
}

export interface AiHealth {
  days: number;
  by_kind: KindCount[];
  by_day: DayIncidents[];
  open_total: number;
  open: OpenIncident[];
  cost: CostHealth;
  tasks: TaskHealth[];
  shadow: ShadowReport;
  budget: { daily_usd: number; used_usd: number; used_pct: number };
}
