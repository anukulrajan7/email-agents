import type { ProcessingStatus } from '../../api/types';
import { cn } from '../../lib/cn';

const stations = ['Received', 'Normalized', 'Classified', 'Workflow done'] as const;

type StationState = 'done' | 'current' | 'failed' | 'upcoming';

function stationStates(status: ProcessingStatus, hasClassification: boolean): StationState[] {
  if (status === 'failed') {
    const failedAt = hasClassification ? 3 : 2;
    return stations.map((_, i) => (i < failedAt ? 'done' : i === failedAt ? 'failed' : 'upcoming'));
  }
  const currentIndex = { pending: 1, normalized: 2, classified: 3, workflow_completed: -1 }[status];
  return stations.map((_, i) => (currentIndex === -1 || i < currentIndex ? 'done' : i === currentIndex ? 'current' : 'upcoming'));
}

const stateLabel: Record<StationState, string> = {
  done: 'done',
  current: 'in progress',
  failed: 'failed',
  upcoming: 'waiting',
};

export function RouteTracker({ status, hasClassification }: { status: ProcessingStatus; hasClassification: boolean }) {
  const states = stationStates(status, hasClassification);
  const reached = states.filter((s) => s === 'done').length - 1;
  const progress = Math.max(0, (reached / (stations.length - 1)) * 100);

  return (
    <ol aria-label="Processing route" className="relative grid grid-cols-4">
      <span aria-hidden="true" className="absolute top-[9px] right-[12.5%] left-[12.5%] h-px bg-rule-strong">
        <span className="absolute inset-y-0 left-0 block h-px bg-ink" style={{ width: `${progress}%` }} />
      </span>
      {stations.map((label, i) => {
        const state = states[i] ?? 'upcoming';
        return (
          <li key={label} aria-current={state === 'current' ? 'step' : undefined} className="relative flex flex-col items-center gap-2 text-center">
            <span
              aria-hidden="true"
              className={cn(
                'flex h-[19px] w-[19px] items-center justify-center rounded-full border-2 bg-paper text-[10px] leading-none font-bold',
                state === 'done' && 'border-ink bg-ink',
                state === 'current' && 'station-pulse border-accent bg-accent',
                state === 'failed' && 'border-accent bg-accent text-accent-ink',
                state === 'upcoming' && 'border-rule-strong',
              )}
            >
              {state === 'failed' ? '✕' : null}
            </span>
            <span className={cn('text-xs', state === 'upcoming' ? 'text-ink-muted' : 'font-semibold text-ink', state === 'failed' && 'text-accent')}>
              {label}
              <span className="sr-only"> ({stateLabel[state]})</span>
            </span>
          </li>
        );
      })}
    </ol>
  );
}
