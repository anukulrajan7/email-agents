export type GoalChoice = { id: string; label: string; pack: string; keywords: string[] };

export const GOALS: GoalChoice[] = [
  { id: 'inbox', label: 'Keep my inbox under control', pack: 'general', keywords: ['inbox', 'triage', 'sort', 'organise', 'organize'] },
  { id: 'support', label: 'Answer customer support', pack: 'support', keywords: ['support', 'ticket', 'customer', 'help desk', 'complaint'] },
  { id: 'sales', label: 'Follow up with leads', pack: 'sales', keywords: ['sales', 'lead', 'prospect', 'quote', 'follow up', 'follow-up'] },
  { id: 'finance', label: 'Chase invoices and payments', pack: 'finance', keywords: ['invoice', 'payment', 'finance', 'billing', 'receipt'] },
  { id: 'personal', label: 'Run my personal mail and calendar', pack: 'personal', keywords: ['personal', 'calendar', 'schedule', 'meeting', 'appointment'] },
  { id: 'hrms', label: 'Run HR and recruiting desks', pack: 'staffing', keywords: ['hr', 'hrms', 'recruit', 'candidate', 'leave', 'payroll', 'timesheet', 'hiring', 'staffing'] },
];

export const PACK_TEAM: Record<string, string[]> = {
  general: ['Meera (lead)', 'Inbox Assistant'],
  support: ['Ticket Resolver', 'Escalations'],
  sales: ['Lead Qualifier', 'Follow-ups'],
  finance: ['Invoices & Payments'],
  personal: ['Scheduling'],
  staffing: ['Leave', 'Timesheets', 'Onboarding', 'Candidates & Requirements', 'Payroll queries'],
};

/** Packs that fit the words in a goal. `general` is always included. */
export function suggestPacks(goal: string): string[] {
  const text = ` ${goal.toLowerCase()} `;
  const hits = GOALS.filter((g) => g.keywords.some((k) => new RegExp(`\\b${k.replace(/[-\s]/g, '[-\\s]')}s?\\b`).test(text))).map((g) => g.pack);
  return Array.from(new Set(['general', ...hits]));
}
