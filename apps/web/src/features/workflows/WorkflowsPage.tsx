import { Link } from 'react-router';
import type { Workflow } from '../../api/types';
import { Badge, Button, Card, CardTitle, EmptyState, Mono, PageHeader, Spinner, Table, Td, Th } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { useWorkspace } from '../../app/useWorkspace';
import { relativeTime } from '../../lib/format';
import { cn } from '../../lib/cn';
import { useReprocess } from '../inbox/hooks';
import { useToggleWorkflow, useWorkflowRuns, useWorkflows } from './hooks';

function Toggle({ workflow, canEdit, onChange, busy }: { workflow: Workflow; canEdit: boolean; onChange: (v: boolean) => void; busy: boolean }) {
  const disabled = !canEdit || !workflow.available || busy;
  return (
    <button
      type="button"
      role="switch"
      aria-checked={workflow.enabled}
      aria-label={`${workflow.name} enabled`}
      disabled={disabled}
      onClick={() => onChange(!workflow.enabled)}
      className={cn(
        'relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors',
        'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50',
        workflow.enabled ? 'bg-success' : 'bg-ink-muted',
      )}
    >
      <span className={cn('inline-block h-5 w-5 rounded-full bg-white shadow-sm transition-transform', workflow.enabled ? 'translate-x-5' : 'translate-x-0.5')} />
    </button>
  );
}

export function WorkflowsPage() {
  const { ws, isAdmin } = useWorkspace();
  const workflows = useWorkflows(ws);
  const runs = useWorkflowRuns(ws);
  const toggle = useToggleWorkflow(ws);
  const reprocess = useReprocess(ws);

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-8 px-10 py-10">
      <PageHeader kicker="Automation" title="Workflows" description="Templates that act on classified emails." />
      {workflows.isLoading && <Spinner />}
      <ErrorNote error={workflows.error ?? toggle.error ?? reprocess.error} />
      <div className="grid gap-4 md:grid-cols-3">
        {workflows.data?.items.map((w, i) => (
          <Card key={w.id} className="rise flex flex-col gap-3" style={riseStyle(i + 1)}>
            <div className="flex items-start justify-between gap-2">
              <CardTitle>{w.name}</CardTitle>
              {!w.available && <Badge>P1</Badge>}
            </div>
            <p className="flex-1 text-sm text-ink-muted">{w.description}</p>
            <div className="flex items-center justify-between gap-2 border-t border-rule pt-3">
              <div className="flex items-center gap-2 text-sm">
                <Toggle
                  workflow={w}
                  canEdit={isAdmin}
                  busy={toggle.isPending && toggle.variables?.id === w.id}
                  onChange={(enabled) => toggle.mutate({ id: w.id, enabled })}
                />
                <span className="text-ink">{w.enabled ? 'Enabled' : 'Disabled'}</span>
              </div>
              {w.last_run ? (
                <span className="flex items-center gap-1 text-xs text-ink-muted">
                  <StatusBadge status={w.last_run.status} /> <Mono>{relativeTime(w.last_run.created_at)}</Mono>
                </span>
              ) : (
                <span className="text-xs text-ink-muted">No runs yet</span>
              )}
            </div>
          </Card>
        ))}
      </div>

      <section aria-labelledby="runs-heading" className="rise flex flex-col gap-3" style={riseStyle(4)}>
        <h2 id="runs-heading" className="type-display text-2xl tracking-[-0.01em]">
          Runs
        </h2>
        {runs.isLoading && <Spinner />}
        <ErrorNote error={runs.error} />
        {runs.data &&
          (runs.data.items.length === 0 ? (
            <Card>
              <EmptyState title="No runs yet" description="Runs appear after an email is classified." />
            </Card>
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>Status</Th>
                  <Th>Message</Th>
                  <Th>Error</Th>
                  <Th>When</Th>
                  {isAdmin && (
                    <Th>
                      <span className="sr-only">Actions</span>
                    </Th>
                  )}
                </tr>
              </thead>
              <tbody>
                {runs.data.items.map((r) => (
                  <tr key={r.id}>
                    <Td>
                      <StatusBadge status={r.status} />
                    </Td>
                    <Td className="max-w-xs">
                      <Link to={`/w/${ws}/inbox/${r.message_id}`} className="block truncate text-accent underline underline-offset-2 hover:text-accent-hover">
                        {r.message_subject || '(no subject)'}
                      </Link>
                    </Td>
                    <Td className="max-w-xs">
                      <span className="line-clamp-2 font-mono text-xs text-danger" title={r.error ?? undefined}>
                        {r.error ?? ''}
                      </span>
                    </Td>
                    <Td className="whitespace-nowrap">
                      <Mono>{relativeTime(r.created_at)}</Mono>
                    </Td>
                    {isAdmin && (
                      <Td className="text-right">
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={reprocess.isPending && reprocess.variables === r.message_id}
                          onClick={() => reprocess.mutate(r.message_id)}
                        >
                          Reprocess
                        </Button>
                      </Td>
                    )}
                  </tr>
                ))}
              </tbody>
            </Table>
          ))}
      </section>
    </div>
  );
}
