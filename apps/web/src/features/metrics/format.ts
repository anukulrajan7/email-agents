export function percent(value: number | null): string {
  return value === null ? '-' : `${Math.round(value * 100)}%`;
}

export function distance(value: number | null): string {
  return value === null ? '-' : value.toFixed(2);
}

export function millis(value: number | null): string {
  if (value === null) return '-';
  return value >= 1000 ? `${(value / 1000).toFixed(1)} s` : `${value} ms`;
}

export function money(value: number | null): string {
  if (value === null) return '-';
  if (value === 0) return '$0.00';
  return value < 0.01 ? `$${value.toFixed(4)}` : `$${value.toFixed(2)}`;
}

export const SUITE_NAMES: Record<string, string> = {
  safety: 'Safety checks',
  replay: 'Replay of past replies',
  golden_live: 'Live check',
  learning_lift: 'Learning check',
};

export const TRIGGER_NAMES: Record<string, string> = {
  version: 'New version',
  schedule: 'Scheduled',
  manual: 'Started by a person',
  promotion: 'Before a promotion',
};

export const LEVEL_NAMES = ['Observe only', 'Draft for approval', 'Send routine replies', 'Send most replies'];

export function shortDate(iso: string): string {
  return new Date(`${iso.slice(0, 10)}T00:00:00Z`).toLocaleDateString('en-GB', { day: 'numeric', month: 'short', timeZone: 'UTC' });
}
