import { formatCost, humanize } from '../../lib/format';
import type { Tone } from '../../components/ui';
import type { CaseFilter, CaseStatus, CaseSummary } from './types';

export const STATUS_LABEL: Record<CaseStatus, string> = {
  running: 'Working',
  waiting_approval: 'Waiting for approval',
  waiting_event: 'Waiting for reply',
  done: 'Done',
  failed: 'Failed',
  cancelled: 'Cancelled',
};

export const STATUS_TONE: Record<CaseStatus, Tone> = {
  running: 'info',
  waiting_approval: 'warning',
  waiting_event: 'neutral',
  done: 'success',
  failed: 'danger',
  cancelled: 'neutral',
};

const OUTCOME_TONE: Record<string, Tone> = {
  submitted: 'info',
  interview: 'success',
  placed: 'success',
  rejected: 'danger',
  closed: 'neutral',
};

export const outcomeLabel = (outcome: string) => humanize(outcome);
export const outcomeTone = (outcome: string): Tone => OUTCOME_TONE[outcome] ?? 'neutral';

export const FILTERS: { id: CaseFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'working', label: 'Working' },
  { id: 'waiting', label: 'Waiting' },
  { id: 'done', label: 'Done' },
  { id: 'failed', label: 'Failed' },
];

export const FILTER_STATUS: Partial<Record<CaseFilter, CaseStatus>> = {
  working: 'running',
  done: 'done',
  failed: 'failed',
};

export const isWaiting = (c: Pick<CaseSummary, 'status'>) => c.status === 'waiting_approval' || c.status === 'waiting_event';
export const isFinished = (status: CaseStatus) => status === 'done' || status === 'failed' || status === 'cancelled';

export const formatCents = formatCost;

export function wakeText(iso: string, now = new Date()): string {
  const d = new Date(iso);
  const time = d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', hour12: false });
  const days = (d.getTime() - now.getTime()) / 86_400_000;
  if (days > 6) return `${d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })} ${time}`;
  return `${d.toLocaleDateString('en-US', { weekday: 'short' })} ${time}`;
}
