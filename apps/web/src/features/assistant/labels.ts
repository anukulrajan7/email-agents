import type { ConfirmLink, PlanAction } from './types';

export const ASSISTANT_NAME = 'Meera';

export const ACTION_LABEL: Record<PlanAction, string> = {
  draft_reply: 'Draft a reply',
  start_agent: 'Start an agent',
  approve: 'Approve',
  decline: 'Decline',
  create_automation: 'New automation',
  snooze_alert: 'Snooze alert',
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
  }
}
