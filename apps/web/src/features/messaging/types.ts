export type InboxItem = {
  id: string;
  thread_key: string;
  kind: 'message' | 'notice';
  subject: string;
  body: string;
  from: string;
  from_assistant: boolean;
  link: { kind?: string; id?: string } | null;
  read: boolean;
  at: string;
  can_reply: boolean;
};

export type SentItem = {
  id: string;
  channel: 'email' | 'in_app';
  to: string;
  subject: string;
  body: string;
  status: string;
  reason?: string | null;
  error?: string | null;
  at: string;
  can_stop: boolean;
  can_approve?: boolean;
};

export type TicketBrief = { id: string; company: string; address: string; status: 'open' | 'waiting' | 'resolved'; last_summary: string };

export type Conversation = {
  id: string;
  kind: 'ask' | 'ticket' | 'desk';
  title: string;
  with: string;
  channel: string;
  status: 'waiting' | 'nudged' | 'answered' | 'open' | 'starting' | 'resolved';
  status_label: string;
  answer: string | null;
  updated_at: string;
  ticket: TicketBrief | null;
};
