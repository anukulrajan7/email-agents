import type { AgentSummary, Autonomy } from '../../api/types';

export type AgentCopy = { role: string; does: string; when: string; example: string };

/** Plain-English copy for each system agent, in its own voice. Keyed by template. */
const BY_TEMPLATE: Record<string, AgentCopy> = {
  'general.lead': {
    role: 'Team lead',
    does: 'I read every email that comes in and hand it to the helper whose job it is. I answer what nobody else owns.',
    when: 'When an email arrives and no other helper has claimed it.',
    example: 'A vendor asks about opening hours. No helper owns that, so I draft the answer for you.',
  },
  'general.inbox': {
    role: 'Inbox assistant',
    does: 'I draft replies to mail that is waiting for an answer.',
    when: 'When an email needs a reply.',
    example: 'A client asks when the report is ready. I draft a short reply for you to check.',
  },
  'personal.mailbox': {
    role: 'Your assistant',
    does: 'I work only in your own mailbox, write as you and follow your profile. Nobody else can see me.',
    when: 'When you ask me to handle an email, or mail arrives that matches your profile.',
    example: 'Priya asks you for a quick call. I draft your reply in your tone and keep it for your OK.',
  },
  'support.resolver': {
    role: 'Ticket resolver',
    does: 'I answer customer questions from what the team already knows, step by step.',
    when: 'When an email is labelled Question, Bug report, Refund request or Customer support.',
    example: 'A customer cannot log in. I find the known fix, draft the steps, and tell you if I am unsure.',
  },
  'support.escalation': {
    role: 'Escalations',
    does: 'I catch complaints and angry customers, ask a person to step in and prepare a calm reply.',
    when: 'When an email is labelled Complaint, or says "escalate", "unacceptable" or "legal action".',
    example: 'A customer threatens to cancel. I flag it to you at once and draft a calm, honest reply. I never send it.',
  },
  'sales.qualifier': {
    role: 'Lead qualifier',
    does: 'I check every new enquiry and tell you which leads are worth a call, with their budget, timeline and need.',
    when: 'When an email is labelled Sales lead.',
    example: 'Acme asks for a quote for 40 seats by March. I note budget, timeline and need, and draft a reply for you.',
  },
  'sales.followups': {
    role: 'Follow-ups',
    does: 'I keep deals moving by nudging proposals and quotes that went quiet, once, politely.',
    when: 'When an email is labelled Follow up, and once a day for conversations that went quiet.',
    example: 'You sent a quote five days ago and heard nothing. I draft a short, friendly nudge.',
  },
  'finance.invoices': {
    role: 'Invoices and payments',
    does: 'I read invoices and payment mail, note the vendor, number, amount and due date, and flag anything odd.',
    when: 'When an email is labelled Invoice, Payment, Expense or Billing.',
    example: 'A vendor sends invoice 1042 for Rs 48,000. I note the details, acknowledge it and ask you to approve.',
  },
  'personal.scheduling': {
    role: 'Scheduling',
    does: 'I answer meeting requests using the free slots I was given, and mark anything I do not know.',
    when: 'When an email is labelled Scheduling, or mentions a meeting, rescheduling or availability.',
    example: 'Priya asks to meet next week. I offer your free slots, or mark the gap if I do not know them.',
  },
  'hrms.leave': {
    role: 'Leave and HR requests',
    does: 'I acknowledge leave requests and HR questions and prepare them for a manager. I never approve anything.',
    when: 'When an email is labelled Leave.',
    example: 'Priya asks for 2 days off. I draft the reply and create a leave task for the manager.',
  },
  'hrms.timesheets': {
    role: 'Timesheets',
    does: 'I confirm each timesheet that arrives and chase hours that are missing or unclear.',
    when: 'When an email is labelled Timesheet.',
    example: 'Ravi sends October hours with two days missing. I confirm receipt and ask him for those two days.',
  },
  'hrms.onboarding': {
    role: 'Onboarding',
    does: 'I welcome new joiners, collect their documents and keep their checklist moving.',
    when: 'When an email mentions onboarding, a new joiner or joining formalities.',
    example: 'A new joiner sends an ID but no bank details. I thank them and ask only for the bank details.',
  },
  'hrms.requirements': {
    role: 'Client requirements',
    does: 'I work a client requirement end to end: shortlist candidates, ask for your approval, submit and follow up.',
    when: 'When an email is labelled Client requirement or Candidate referral.',
    example: 'A client needs a Java lead in Hyderabad. I shortlist three candidates and ask you before submitting.',
  },
  'hrms.payroll': {
    role: 'Payroll questions',
    does: 'I acknowledge payroll questions and hand them to the payroll owner. I never state an amount.',
    when: 'When an email is labelled Payroll.',
    example: 'Priya asks about her payslip. I reply that payroll will check, and pass it on with her question.',
  },
};

const BY_CATEGORY: Record<AgentSummary['category'], string> = {
  general: 'General',
  support: 'Support',
  sales: 'Sales',
  finance: 'Finance',
  personal: 'Personal',
  hrms: 'HRMS',
  custom: 'Custom helper',
};

const GENERIC: Pick<AgentCopy, 'when' | 'example'> = {
  when: 'When an email arrives that matches the rules this helper was set up with.',
  example: 'A matching email arrives. It follows its steps, and anything beyond its freedom level comes to you.',
};

export function agentCopy(agent: Pick<AgentSummary, 'template_key' | 'category' | 'description'>): AgentCopy {
  const known = agent.template_key ? BY_TEMPLATE[agent.template_key] : undefined;
  if (known) return known;
  return { role: BY_CATEGORY[agent.category], does: agent.description.trim() || 'Looks after one kind of email.', ...GENERIC };
}

export const AUTONOMY_PLAIN: Record<Autonomy, { headline: string; alone: string; asks: string }> = {
  0: {
    headline: 'Watches only: learning, never acts',
    alone: 'Watches your mail and learns how you handle it.',
    asks: 'Nothing yet. It never drafts, sends or changes anything.',
  },
  1: {
    headline: 'Drafts replies, you approve',
    alone: 'Reads mail and writes draft replies.',
    asks: 'Every reply, before anything is sent.',
  },
  2: {
    headline: 'Sends routine replies after a 10-minute hold',
    alone: 'Sends routine replies after a 10-minute hold, and you can press Stop in that time.',
    asks: 'Anything that is not routine, new or sensitive.',
  },
  3: {
    headline: 'Replies on its own after a 10-minute hold',
    alone: 'Replies by itself after a 10-minute hold, and you can press Stop in that time.',
    asks: 'Only what its job description says a person must decide.',
  },
};

const LEGACY_PERSONAL = /^(my assistant|.*\(personal\))$/i;

/** The name people see: the personal agent reads "Your Meera", never a placeholder. */
export function agentDisplayName(agent: { name: string; scope?: string }): string {
  if (agent.scope !== 'personal') return agent.name;
  const base = LEGACY_PERSONAL.test(agent.name.trim()) ? 'Meera' : agent.name.trim();
  return `Your ${base}`;
}
