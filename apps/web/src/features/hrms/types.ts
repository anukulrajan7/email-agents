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
