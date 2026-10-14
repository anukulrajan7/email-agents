import { Badge, Button, EmptyState, Mono, RelativeTime, Spinner, Table, Td, Th, type Tone } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useEvals, useStartEval } from './hooks';
import { SUITE_NAMES, TRIGGER_NAMES, money, percent } from './format';
import type { EvalRun } from './types';

function statusTone(run: EvalRun): Tone {
  if (run.status === 'queued' || run.status === 'running') return 'info';
  if (run.status === 'error') return 'danger';
  return run.status === 'passed' ? 'success' : 'warning';
}

function statusText(run: EvalRun): string {
  if (run.status === 'queued') return 'Waiting';
  if (run.status === 'running') return 'Running';
  if (run.status === 'error') return 'Did not finish';
  return run.status === 'passed' ? 'Passed' : 'Needs attention';
}

function Change({ run }: { run: EvalRun }) {
  if (run.score === null || run.baseline_score === null) return <Mono>-</Mono>;
  const delta = Math.round((run.score - run.baseline_score) * 100);
  if (delta === 0) return <Mono>No change</Mono>;
  return <Mono className={delta > 0 ? 'text-success' : 'text-danger'}>{delta > 0 ? `+${delta}` : delta} points</Mono>;
}

export function EvalsPanel({ ws, agentId, canRun }: { ws: string; agentId: string; canRun: boolean }) {
  const evals = useEvals(ws, agentId);
  const start = useStartEval(ws, agentId);
  const busy = start.isPending || evals.data?.items.some((r) => r.status === 'queued' || r.status === 'running');

  return (
    <section className="flex flex-col gap-4" aria-labelledby="evals-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="evals-heading" className="type-display text-xl tracking-[-0.01em] text-ink">
            Checks
          </h2>
          <p className="mt-1 max-w-[62ch] text-sm text-ink-muted">
            Safety checks run on every new version and must pass before it keeps any autonomy. Replay compares the agent with replies people already approved.
          </p>
        </div>
        {canRun && (
          <div className="flex flex-wrap gap-2">
            <Button variant="secondary" size="sm" disabled={busy} onClick={() => start.mutate('safety')}>
              Run safety checks
            </Button>
            <Button variant="secondary" size="sm" disabled={busy} onClick={() => start.mutate('replay')}>
              Replay past replies
            </Button>
            {evals.data?.live_available && (
              <Button variant="secondary" size="sm" disabled={busy} onClick={() => start.mutate('golden_live')}>
                Run live check
              </Button>
            )}
          </div>
        )}
      </div>
      <ErrorNote error={start.error ?? evals.error} />
      {evals.isLoading && <Spinner />}
      {evals.data && evals.data.items.length === 0 && (
        <EmptyState title="No checks yet" description="Run the safety checks to see how this agent behaves on tricky emails." />
      )}
      {evals.data && evals.data.items.length > 0 && (
        <Table>
          <caption className="sr-only">Check history, newest first</caption>
          <thead>
            <tr>
              <Th>Check</Th>
              <Th>Result</Th>
              <Th className="text-right">Passed</Th>
              <Th className="text-right">Score</Th>
              <Th>Change</Th>
              <Th className="text-right">Version</Th>
              <Th className="text-right">Cost</Th>
              <Th>When</Th>
            </tr>
          </thead>
          <tbody>
            {evals.data.items.map((run) => (
              <tr key={run.id}>
                <Td>
                  <span className="font-semibold">{SUITE_NAMES[run.suite] ?? run.suite}</span>
                  <br />
                  <Mono>{TRIGGER_NAMES[run.trigger] ?? run.trigger}</Mono>
                </Td>
                <Td>
                  <Badge tone={statusTone(run)}>{statusText(run)}</Badge>
                </Td>
                <Td className="text-right">
                  <Mono>
                    {run.cases_passed} of {run.cases_total}
                  </Mono>
                </Td>
                <Td className="text-right">
                  <Mono>{percent(run.score)}</Mono>
                </Td>
                <Td>
                  <Change run={run} />
                </Td>
                <Td className="text-right">
                  <Mono>{run.agent_version ?? '-'}</Mono>
                </Td>
                <Td className="text-right">
                  <Mono>{money(run.cost_usd)}</Mono>
                </Td>
                <Td>
                  <RelativeTime iso={run.created_at} className="text-ink-muted" />
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </section>
  );
}
