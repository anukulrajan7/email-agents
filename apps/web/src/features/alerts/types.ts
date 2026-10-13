export type AlertKind = 'important_unanswered' | 'approval_pending' | 'automation';
export type AlertSeverity = 'high' | 'normal';
export type AlertStatus = 'open' | 'snoozed' | 'resolved';
export type AlertFilter = 'open' | 'resolved' | 'all';

export type Alert = {
  id: string;
  kind: AlertKind;
  severity: AlertSeverity;
  title: string;
  body: string;
  message_id: string | null;
  message_subject: string | null;
  status: AlertStatus;
  escalation_level: number;
  notified: string[];
  created_at: string;
  resolved_at: string | null;
};

export type AlertList = { items: Alert[]; open_count?: number };

export type AlertPatch = { status: 'resolved' | 'open'; snooze_hours?: number };

export type AlertSettings = {
  enabled: boolean;
  recipients: string[];
  escalate_to: string[];
  unanswered_hours: number;
  escalate_after_hours: number;
  daily_digest: boolean;
  digest_hour: number;
};

export type AlertTestResult = { sent_to: string[] };
