import { Card, CardTitle, SegmentedControl, Skeleton } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { useFlags, useSetFlag } from './hooks';
import type { FlagMode } from './types';

const MODES: { value: FlagMode; label: string }[] = [
  { value: 'off', label: 'Off' },
  { value: 'compare', label: 'Compare' },
  { value: 'on', label: 'On' },
];

export function FlagsPanel() {
  const { ws, isAdmin } = useWorkspace();
  const flags = useFlags(ws, isAdmin);
  const set = useSetFlag(ws);

  if (!isAdmin) return null;

  return (
    <section className="flex flex-col gap-3" aria-labelledby="flags-title">
      <CardTitle id="flags-title">New behaviour</CardTitle>
      <p className="text-sm text-ink-muted">
        Off keeps things as they are today. Compare runs the new way beside today's and only records what it would have done. On lets the new way decide.
      </p>
      {flags.isLoading && <Skeleton className="h-24" />}
      <ErrorNote error={flags.error} />
      <ErrorNote error={set.error} />
      {flags.data && (
        <Card className="p-0">
          <ul className="flex flex-col divide-y divide-rule">
            {flags.data.items.map((flag) => (
              <li key={flag.key} className="flex flex-wrap items-center justify-between gap-4 p-4">
                <div className="flex min-w-0 flex-1 basis-64 flex-col gap-1">
                  <p className="font-mono text-sm font-semibold text-ink">{flag.key}</p>
                  <p className="text-sm text-ink-muted">{flag.description}</p>
                </div>
                <SegmentedControl
                  label={`${flag.key} mode`}
                  options={MODES}
                  value={flag.mode}
                  onChange={(mode) => {
                    if (mode !== flag.mode && !set.isPending) set.mutate({ key: flag.key, mode });
                  }}
                />
              </li>
            ))}
          </ul>
        </Card>
      )}
    </section>
  );
}
