export type AccessLevel = 'super_admin' | 'admin' | 'operator';

export type TeamAccess = {
  level: AccessLevel;
  person: { id: string; name: string; role: string | null } | null;
  is_manager: boolean;
  report_count: number;
  staffing: boolean;
  primary_metric: string;
};

export type MetricUnit = 'count' | 'rate' | 'minutes';

export type MetricValue = {
  key: string;
  label: string;
  unit: MetricUnit;
  primary: boolean;
  today: number | null;
  week: number | null;
  since_joining: number | null;
};

export type PersonMetrics = {
  person_id: string;
  name: string;
  role: string | null;
  joining_date: string | null;
  primary_metric: string;
  daily_target: number | null;
  weekly_target: number | null;
  today: number;
  week: number;
  metrics: MetricValue[];
};

export type TeamMetrics = {
  manager_id: string | null;
  primary_metric: string;
  today: number;
  week: number;
  daily_target: number;
  weekly_target: number;
  metrics: MetricValue[];
  members: PersonMetrics[];
};

export type MyMetrics = { person: { id: string; name: string } | null; metrics: PersonMetrics | null };

export type EmployeeType = 'billable' | 'non_billable';

export type Person = {
  id: string;
  name: string;
  emails: string[];
  employee_type?: EmployeeType;
  client?: string | null;
  manager_id: string | null;
  role: string | null;
  joining_date: string | null;
  daily_target: number | null;
  weekly_target: number | null;
  source_id: string | null;
  active: boolean;
};

export type PeopleList = { items: Person[]; can_edit: boolean };

export type PersonInput = {
  name: string;
  emails: string[];
  role: string | null;
  manager_id: string | null;
  joining_date: string | null;
  daily_target: number | null;
  weekly_target: number | null;
  employee_type?: EmployeeType;
  client?: string | null;
};

export type ImportReport = {
  dry_run: boolean;
  total_rows: number;
  created: number;
  updated: number;
  skipped: number;
  errors: { row: number; field: string; message: string }[];
};

export type EscalationStep = { level: number; label: string; name: string | null; emails: string[] };

export type Escalations = {
  manager_after_hours: number;
  final_after_hours: number;
  final_emails: string[];
  chains: { person: { id: string; name: string; role: string | null }; steps: EscalationStep[] }[];
  can_edit: boolean;
};

export type TeamSettings = {
  tz_offset_minutes: number;
  manager_after_hours: number | null;
  final_after_hours: number | null;
  final_emails: string[];
  below_target_hour: number;
  weekly_check_hour: number;
  route_alerts_by_owner?: boolean;
  restrict_mail_to_team?: boolean;
  hr_source_id?: string | null;
  client_domains?: string[];
  timesheet_grace_days?: number;
  hours_tolerance?: number;
  sources?: { id: string; kind: string; mailbox: string | null }[];
};
