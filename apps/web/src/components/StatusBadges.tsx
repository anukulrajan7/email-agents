import type { ProcessingStatus, RunStatus, SourceStatus, ReviewStatus, DraftStatus } from '../api/types';
import { humanize } from '../lib/format';
import { Badge, type Tone } from './ui';

const tones: Record<string, Tone> = {
  pending: 'neutral',
  normalized: 'info',
  classified: 'info',
  workflow_running: 'info',
  processed: 'success',
  workflow_completed: 'success',
  failed: 'danger',
  running: 'info',
  completed: 'success',
  skipped: 'neutral',
  connected: 'success',
  degraded: 'warning',
  disconnected: 'danger',
  open: 'warning',
  closed: 'success',
  suggested: 'info',
  edited: 'warning',
  approved: 'success',
  sent: 'success',
};

const labels: Record<string, string> = {
  pending: 'Waiting',
  normalized: 'Reading',
  classified: 'Labelled',
  workflow_running: 'Following up',
  workflow_completed: 'Done',
  processed: 'Done',
  running: 'In progress',
  completed: 'Done',
  degraded: 'Needs attention',
  closed: 'Done',
  suggested: 'Suggested',
};

type AnyStatus = ProcessingStatus | RunStatus | SourceStatus | ReviewStatus | DraftStatus;

export function statusLabel(status: string): string {
  return labels[status] ?? humanize(status);
}

export function StatusBadge({ status }: { status: AnyStatus }) {
  return <Badge tone={tones[status] ?? 'neutral'}>{statusLabel(status)}</Badge>;
}
