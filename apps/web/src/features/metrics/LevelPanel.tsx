import { Badge, Button, Card, CardTitle, RelativeTime, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { useAutonomy, useChangeLevel } from './hooks';
import { LEVEL_NAMES } from './format';

export function LevelPanel({ ws, agentId, canChange }: { ws: string; agentId: string; canChange: boolean }) {
  const autonomy = useAutonomy(ws, agentId);
  const change = useChangeLevel(ws, agentId);
  const state = autonomy.data;
  const next = state?.next;
  const ready = next?.requirements.every((r) => r.met) ?? false;

  return (
    <section className="flex flex-col gap-4" aria-labelledby="level-heading">
      <h2 id="level-heading" className="type-display text-xl tracking-[-0.01em] text-ink">
        Why this level
      </h2>
      {autonomy.isLoading && <Spinner />}
      <ErrorNote error={change.error ?? autonomy.error} />
      {state && (
        <>
          <Card className="flex flex-col gap-3">
            <p className="type-display text-2xl tracking-[-0.01em] text-ink">
              Level {state.earned}: {state.level_name}
            </p>
            <ul className="flex list-disc flex-col gap-1 pl-5 text-sm text-ink-muted">
              {state.why.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
            {canChange && state.earned > 0 && (
              <div>
                <Button variant="secondary" size="sm" disabled={change.isPending} onClick={() => change.mutate(state.earned - 1)}>
                  Lower to level {state.earned - 1}
                </Button>
              </div>
            )}
          </Card>
          {next && (
            <Card className="flex flex-col gap-3">
              <CardTitle>
                Next: level {next.level}, {next.name}
              </CardTitle>
              <ul className="flex flex-col divide-y divide-rule" aria-label={`Requirements for level ${next.level}`}>
                {next.requirements.map((r) => (
                  <li key={r.name} className="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
                    <span className="flex items-center gap-2">
                      <Badge tone={r.met ? 'success' : 'warning'}>{r.met ? 'Met' : 'Not yet'}</Badge>
                      <span className="text-ink">{r.name}</span>
                    </span>
                    <span className={cn('font-mono text-xs', r.met ? 'text-ink-muted' : 'text-review-ink')}>
                      needs {r.target}, now {r.actual}
                    </span>
                  </li>
                ))}
              </ul>
              {canChange && (
                <div>
                  <Button size="sm" disabled={change.isPending || !ready} onClick={() => change.mutate(next.level)}>
                    Promote to level {next.level}
                  </Button>
                </div>
              )}
            </Card>
          )}
          {state.history.length > 0 && (
            <div className="flex flex-col gap-2">
              <CardTitle>History</CardTitle>
              <ol className="flex flex-col divide-y divide-rule rounded-sm border border-rule bg-surface">
                {state.history.map((h) => (
                  <li key={`${h.created_at}-${h.to}`} className="flex flex-wrap items-baseline justify-between gap-2 px-4 py-2 text-sm">
                    <span className="text-ink">
                      {LEVEL_NAMES[h.from] ?? h.from} to {LEVEL_NAMES[h.to] ?? h.to}
                      <span className="text-ink-muted">
                        {' '}
                        ({h.actor === 'system' ? 'automatic' : 'by a person'}): {h.reason}
                      </span>
                    </span>
                    <RelativeTime iso={h.created_at} className="font-mono text-xs text-ink-muted" />
                  </li>
                ))}
              </ol>
            </div>
          )}
        </>
      )}
    </section>
  );
}
