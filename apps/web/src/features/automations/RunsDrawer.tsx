import { Link } from 'react-router';
import { Badge, EmptyState, Mono, Spinner, type Tone } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { humanize, relativeTime } from '../../lib/format';
import { Overlay } from './Overlay';
import { useAutomationRuns } from './hooks';
import type { Automation, AutomationActionStatus, AutomationRunStatus } from './types';

const runTone: Record<AutomationRunStatus, Tone> = { completed: 'success', skipped: 'neutral', failed: 'danger' };
const actionTone: Record<AutomationActionStatus, Tone> = { done: 'success', skipped: 'neutral', failed: 'danger' };

export function RunsDrawer({ ws, automation, onClose }: { ws: string; automation: Automation; onClose: () => void }) {
  const runs = useAutomationRuns(ws, automation.id);
  return (
    <Overlay variant="drawer" title={`Runs: ${automation.name}`} titleId="runs-title" onClose={onClose}>
      {runs.isLoading && <Spinner />}
      <ErrorNote error={runs.error} />
      {runs.data &&
        (runs.data.items.length === 0 ? (
          <EmptyState title="No runs yet" description="Each time this automation handles an email, it is listed here with what happened to every step." />
        ) : (
          <ul className="flex flex-col divide-y divide-rule" aria-label="Runs">
            {runs.data.items.map((r) => (
              <li key={r.id} className="flex flex-col gap-2 py-3">
                <div className="flex items-start justify-between gap-3">
                  <Link to={`/w/${ws}/inbox/${r.message_id}`} className="min-w-0 truncate text-sm font-semibold text-accent underline underline-offset-2 hover:text-accent-hover">
                    {r.message_subject || '(no subject)'}
                  </Link>
                  <div className="flex shrink-0 items-center gap-2">
                    <Badge tone={runTone[r.status]}>{r.status}</Badge>
                    <Mono>{relativeTime(r.created_at)}</Mono>
                  </div>
                </div>
                <ul className="flex flex-col gap-1" aria-label="Steps">
                  {r.actions.map((a, i) => (
                    <li key={i} className="flex flex-wrap items-center gap-2 text-sm">
                      <Badge tone={actionTone[a.status]}>{a.status}</Badge>
                      <span className="text-ink">{humanize(a.type)}</span>
                      {a.detail && <span className="text-ink-muted">{a.detail}</span>}
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        ))}
    </Overlay>
  );
}
