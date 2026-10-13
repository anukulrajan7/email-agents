import type { Action, AutomationInput } from './types';

export type Issue = { id: string; message: string };

export const DEFAULT_DAILY_CAP = 20;
export const DEFAULT_REMIND_HOURS = 4;

export function needsEarlierDraft(actions: Action[], index: number): boolean {
  const action = actions[index];
  if (action?.type !== 'save_to_gmail_drafts' && action?.type !== 'send_reply') return false;
  return !actions.slice(0, index).some((a) => a.type === 'draft_reply');
}

const positiveInt = (n: number) => Number.isInteger(n) && n >= 1;

const DOMAIN = /^(?=.{3,253}$)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;

export function normalizeDomain(raw: string): string {
  return raw.trim().toLowerCase();
}

export function domainError(raw: string): string | undefined {
  const d = normalizeDomain(raw);
  if (d.includes('@')) return 'Enter just the domain, like acme.com, without “@”.';
  if (!DOMAIN.test(d)) return `“${raw.trim()}” is not a valid domain. Use something like acme.com.`;
  return undefined;
}

export function validateAutomation(input: AutomationInput): Issue[] {
  const issues: Issue[] = [];
  if (input.name.trim() === '') issues.push({ id: 'name', message: 'Give this automation a name.' });
  if (input.actions.length === 0) issues.push({ id: 'actions', message: 'Add at least one step under “Then…”.' });
  input.conditions.forEach((c, i) => {
    if (typeof c.value === 'string' && c.value.trim() === '') {
      issues.push({ id: `condition-${i}`, message: `Condition ${i + 1} needs a value.` });
    }
  });
  input.actions.forEach((a, i) => {
    if (needsEarlierDraft(input.actions, i)) {
      const what = a.type === 'send_reply' ? 'Send the reply' : 'Save to Gmail drafts';
      issues.push({ id: `action-${i}`, message: `Step ${i + 1} (${what}) needs “Draft a reply” before it.` });
    }
    if (a.type === 'send_reply' && !positiveInt(a.daily_cap)) {
      issues.push({ id: `action-${i}-cap`, message: `Step ${i + 1}: the daily cap must be a whole number of at least 1.` });
    }
    if (a.type === 'send_reply') {
      if (a.allowed_domains.length === 0) {
        issues.push({ id: `action-${i}-domains`, message: `Step ${i + 1}: add at least one sender domain to auto-send to, like acme.com.` });
      } else if (a.allowed_domains.some((d) => domainError(d))) {
        issues.push({ id: `action-${i}-domains`, message: `Step ${i + 1}: one of the sender domains is not valid.` });
      }
    }
    if (a.type === 'remind_if_no_reply' &&!positiveInt(a.hours)) {
      issues.push({ id: `action-${i}-hours`, message: `Step ${i + 1}: reminder hours must be a whole number of at least 1.` });
    }
  });
  return issues;
}
