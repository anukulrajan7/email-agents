export type LinkOption = { id: string; name?: string; role_title?: string; client?: string | null; score: number };

export type Submission = {
  id: string;
  person_id: string;
  person_name: string;
  sent_at: string;
  client_domain: string;
  client: string | null;
  attachment_name: string;
  candidate_name: string | null;
  candidate_id: string | null;
  requirement_id: string | null;
  requirement_title: string | null;
  link_status: 'auto' | 'needs_confirm' | 'confirmed' | 'unlinked';
  options: { candidates?: LinkOption[]; requirements?: LinkOption[] };
  status: 'active' | 'dismissed';
  outcome: 'pending' | 'interview' | 'rejected' | 'selected' | 'joined';
  reached_interview: boolean;
};

export type SubmissionList = { items: Submission[]; needs_confirm: number };

export type TimesheetRow = {
  person_id: string;
  name: string;
  month: string;
  status: 'received' | 'pending' | 'missing';
  due_at: string;
  timesheet: { message_id: string; subject: string; received_at: string; hours: number | null } | null;
};

export type TimesheetsView = { month: string; grace_days: number; hr_mailbox_set: boolean; items: TimesheetRow[]; missing: number };

export type Mismatch = {
  id: string;
  person_id: string;
  person_name: string;
  month: string;
  kind: string;
  detail: Record<string, unknown>;
  status: 'open' | 'resolved';
  updated_at: string;
};

export type MismatchList = { items: Mismatch[]; portal_hours_rows: number; portal_submission_rows: number };

export type GridDay = { date: string; weekday: string; weekend: boolean };

export type GridStatus = 'recorded' | 'needs_check' | 'missing' | 'pending';

export type GridRow = {
  person_id: string | null;
  name: string;
  employee_code: string | null;
  client: string | null;
  cells: Record<string, number>;
  total: number;
  status: GridStatus;
  record_ids: string[];
  /** A name nobody on the roster owns: kept and shown, never counted for a person. */
  unresolved?: boolean;
};

export type TimesheetGrid = {
  month: string;
  days: GridDay[];
  rows: GridRow[];
  due_at: string;
  summary: { recorded: number; needs_check: number; missing: number; unresolved?: number };
  roster_empty?: boolean;
};

export type RecordStatus = 'recorded' | 'replaced' | 'needs_check';

export type RecordIssue = { code: string; message: string; severity: 'check' | 'note' };

export type RecordEntry = { date: string | null; hours: number; project?: string; note?: string };

export type TimesheetRecord = {
  id: string;
  person_id: string | null;
  person_name: string | null;
  name_as_written: string | null;
  employee_code: string | null;
  client: string | null;
  period_start: string;
  period_end: string;
  total_hours: number | null;
  status: RecordStatus;
  source: 'pasted' | 'attachment';
  source_label: string;
  attachment_filename: string | null;
  match_method: 'code' | 'alias' | 'email' | 'name' | 'manual' | 'none';
  match_confidence: number | null;
  needs_person: boolean;
  issues: RecordIssue[];
  replaces_id: string | null;
  email: { message_id: string | null; available: boolean; subject: string | null; from: string | null; received_at: string | null };
  created_at: string;
  updated_at: string;
  entries?: RecordEntry[];
  identity?: { code?: string | null; name?: string | null; email?: string | null; client?: string | null; manager?: string | null; location?: string | null };
};

export type RecordFilters = { q?: string; month?: string; status?: string; person_id?: string };

export type IdentitySuggestion = { person_id: string; name: string; reason: string };

export type UnknownSheet = {
  id: string;
  period_start: string;
  period_end: string;
  total_hours: number | null;
  file: string | null;
  subject: string | null;
  received_at: string | null;
  message_id: string | null;
  possible_duplicate: boolean;
};

/** A name that came on timesheets and is not on the roster, with the sheets that wait for it. */
export type UnknownPerson = {
  id: string;
  name: string | null;
  employee_code: string | null;
  sender: string | null;
  sender_domain: string | null;
  record_count: number;
  records: UnknownSheet[];
  suggestions: IdentitySuggestion[];
  updated_at: string;
};

export type IdentityChange = {
  id: string;
  kind: 'link' | 'unlink' | 'create_person' | 'mark_external' | 'dismiss' | 'reopen' | 'alias' | 'fill_field' | 'add_email';
  person_id: string | null;
  person_name: string | null;
  name: string | null;
  record_count: number;
  by: string;
  undone: boolean;
  can_undo: boolean;
  at: string;
};

export type IdentityList = { enabled: boolean; items: UnknownPerson[]; recent: IdentityChange[] };

export type NewPersonBody = { name?: string; email?: string; employee_code?: string };
