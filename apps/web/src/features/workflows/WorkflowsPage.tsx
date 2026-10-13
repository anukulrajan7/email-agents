import { Link } from 'react-router';
import type { Workflow } from '../../api/types';
import { Badge, Button, Card, CardTitle, EmptyState, Mono, PageHeader, Spinner, Table, Td, Th, Page } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { useWorkspace } from '../../app/useWorkspace';
import { relativeTime } from '../../lib/format';
import { cn } from '../../lib/cn';
import { useLabelMap } from '../labels/hooks';
import { useReprocess } from '../inbox/hooks';
import { useSetupWorkflow, useToggleWorkflow, useWorkflowRuns, useWorkflows } from './hooks';

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

function setupLabelName(w: Workflow): string {
  const names: Record<string, string> = { timesheet: 'Timesheet', leave: 'Leave', candidate_referral: 'Candidate referral' };
  return names[w.label_key] ?? names[w.template_key.replace(/_(review|request)$/, '')] ?? w.label_key;
}

export function WorkflowsPage() {
  const { ws, isAdmin } = useWorkspace();
  const workflows = useWorkflows(ws);
  const labelMap = useLabelMap(ws);
  const runs = useWorkflowRuns(ws);
  const toggle = useToggleWorkflow(ws);
  const reprocess = useReprocess(ws);
  const setup = useSetupWorkflow(ws);

  return (
    <Page>
      <PageHeader kicker="Automation" title="Workflows" description="Automations that run when an email gets a certain label — for example, creating a review task." />
      {workflows.isLoading && <Spinner />}
      <ErrorNote error={workflows.error ?? toggle.error ?? reprocess.error ?? setup.error} />
      <div className="grid gap-4 md:grid-cols-3 2xl:grid-cols-4">
        {workflows.data?.items.map((w, i) => (
          <Card key={w.id} className="rise flex flex-col gap-3" style={riseStyle(i + 1)}>
            <div className="flex items-start justify-between gap-2">
              <CardTitle>{w.name}</CardTitle>
              {!w.available && <Badge>Needs label</Badge>}
            </div>
            <p className="text-sm text-ink-muted">{w.description}</p>
            <div className="flex-1 text-sm">
              <p>
                <span className="font-semibold">Runs on label:</span> {labelMap.get(w.label_key)?.name ?? w.label_key}
              </p>
              {!w.available &&
                (isAdmin ? (
                  <div className="mt-2 flex flex-col items-start gap-1">
                    <Button size="sm" loading={setup.isPending && setup.variables === w.id} onClick={() => setup.mutate(w.id)}>
                      Add “{setupLabelName(w)}” label &amp; enable
                    </Button>
                    <p className="text-ink-muted">Creates the label the AI uses to spot these emails, then turns the workflow on.</p>
                  </div>
                ) : (
                  <p className="mt-1 text-ink-muted">Ask a workspace admin to add the label for this workflow.</p>
                ))}
            </div>
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
              <EmptyState title="No runs yet" description="Each time an enabled workflow handles an email, the run is listed here. Turn a workflow on above, then wait for a matching email." />
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
    </Page>
  );
}
