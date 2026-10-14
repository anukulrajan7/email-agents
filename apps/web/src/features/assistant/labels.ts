import type { ConfirmLink, PlanAction } from './types';

export const ASSISTANT_NAME = 'Meera';

export const ACTION_LABEL: Record<PlanAction, string> = {
  draft_reply: 'Draft a reply',
  start_agent: 'Start an agent',
  approve: 'Approve',
  decline: 'Decline',
  create_automation: 'New automation',
  snooze_alert: 'Snooze alert',
  send_message: 'Send a message',
  contact_support: 'Write to support',
  hand_to_desk: 'Hand to the team',
};

export function linkTarget(ws: string, link: ConfirmLink): { to: string; label: string } {
  switch (link.kind) {
    case 'message':
      return { to: `/w/${ws}/inbox/${link.id ?? ''}`, label: 'Open the email' };
    case 'case':
      return { to: `/w/${ws}/agents/${link.id ?? ''}`, label: 'Open the agent' };
    case 'approvals':
      return { to: `/w/${ws}/agents`, label: 'Open Agents' };
    case 'automations':
      return { to: `/w/${ws}/automations`, label: 'Open Automations' };
    case 'alerts':
      return { to: `/w/${ws}/alerts`, label: 'Open Alerts' };
    case 'sent':
      return { to: `/w/${ws}/messages`, label: 'See what I sent' };
    case 'inbox':
    case 'meera':
      return { to: `/w/${ws}/messages`, label: 'Open Messages' };
  }
}

const DAY_MS = 86_400_000;

/** "Today", "Yesterday" or a short date, for the dividers between days of conversation. */
export function dayLabel(iso: string, now = new Date()): string {
  const then = new Date(iso);
  if (Number.isNaN(then.getTime())) return '';
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(now) - startOf(then)) / DAY_MS);
  if (days === 0) return 'Today';
  if (days === 1) return 'Yesterday';
  const sameYear = then.getFullYear() === now.getFullYear();
  return then.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', ...(sameYear ? {} : { year: 'numeric' }) });
}

export const dayKey = (iso: string) => new Date(iso).toDateString();
