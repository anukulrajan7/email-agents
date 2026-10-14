export interface AgentIdentity {
  id: string;
  key: string;
  kind: string;
  scope: string;
  category: string;
  name: string;
  avatar: string;
  color: string;
  status: string;
  active_version: number | null;
}

export interface Totals {
  routed: number;
  cases: number;
  success_rate: number | null;
  approval_rate: number | null;
  decline_rate: number | null;
  approvals_decided: number;
  edit_distance_median: number | null;
  auto_sends: number;
  stop_rate: number | null;
  label_accuracy: number | null;
  shadow_agreement: number | null;
  cost_usd: number;
  cost_per_email_usd: number | null;
  cost_per_case_usd: number | null;
  latency_p50_ms: number | null;
  latency_p95_ms: number | null;
  incidents: number;
}

export interface SeriesPoint {
  date: string;
  approval_rate: number | null;
  edit_distance: number | null;
  cost_usd: number;
  cases: number;
}

export interface AgentMetrics {
  days: number;
  agent: AgentIdentity;
  totals: Totals;
  series: SeriesPoint[];
}

export interface ScorecardItem {
  agent: AgentIdentity;
  totals: Totals;
  autonomy: { earned: number; cap: number; ready_for: number | null };
  safety: { status: string; score: number | null; created_at: string } | null;
}

export interface Scorecard {
  days: number;
  totals: Totals;
  agents: ScorecardItem[];
}

export interface EvalRun {
  id: string;
  suite: 'safety' | 'replay' | 'golden_live' | 'learning_lift';
  trigger: 'version' | 'schedule' | 'manual' | 'promotion';
  status: string;
  score: number | null;
  cases_total: number;
  cases_passed: number;
  baseline_score: number | null;
  agent_version: number | null;
  cost_usd: number;
  note: string | null;
  created_at: string;
  finished_at: string | null;
}

export interface VersionInfo {
  version: number;
  source: string;
  gate_status: string;
  note: string | null;
  active: boolean;
  created_at: string;
}

export interface EvalList {
  items: EvalRun[];
  versions: VersionInfo[];
  live_available: boolean;
}

export interface Requirement {
  name: string;
  target: string;
  actual: string;
  met: boolean;
}

export interface AutonomyChange {
  from: number;
  to: number;
  reason: string;
  actor: 'system' | 'person';
  created_at: string;
}

export interface AutonomyState {
  earned: number;
  cap: number;
  effective: number;
  limited_by: string;
  level_name: string;
  why: string[];
  version: { number: number; gate_status: string | null };
  next: { level: number; name: string; requirements: Requirement[] } | null;
  history: AutonomyChange[];
}
