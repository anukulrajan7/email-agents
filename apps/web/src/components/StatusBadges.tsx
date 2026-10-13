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

type AnyStatus = ProcessingStatus | RunStatus | SourceStatus | ReviewStatus | DraftStatus;

export function StatusBadge({ status }: { status: AnyStatus }) {
  return <Badge tone={tones[status] ?? 'neutral'}>{humanize(status)}</Badge>;
}
