import type { AlertSettings } from './types';

const EMAIL = /^[^\s@,;]+@[^\s@,;]+\.[^\s@,;]+$/;

export const isEmail = (value: string) => EMAIL.test(value);

export type SettingsErrors = Partial<Record<'recipients' | 'unanswered_hours' | 'escalate_after_hours', string>>;

const isWholeHours = (n: number) => Number.isInteger(n) && n >= 1 && n <= 720;

export function validateSettings(s: AlertSettings): SettingsErrors {
  const errors: SettingsErrors = {};
  if (s.enabled && s.recipients.length === 0) errors.recipients = 'Add at least one person to notify.';
  if (!isWholeHours(s.unanswered_hours)) errors.unanswered_hours = 'Enter a whole number of hours, 1 to 720.';
  if (!isWholeHours(s.escalate_after_hours)) errors.escalate_after_hours = 'Enter a whole number of hours, 1 to 720.';
  return errors;
}

export function hoursUntilTomorrowMorning(now = new Date()): number {
  const target = new Date(now);
  target.setDate(target.getDate() + 1);
  target.setHours(9, 0, 0, 0);
  return Math.max(1, Math.ceil((target.getTime() - now.getTime()) / 3_600_000));
}
