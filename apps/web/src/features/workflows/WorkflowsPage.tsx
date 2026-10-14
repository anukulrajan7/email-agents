import { Link } from 'react-router';
import type { Workflow } from '../../api/types';
import { statusLabel } from '../../components/StatusBadges';
import { Badge, Button, Card, CardTitle, EmptyState, Mono, Page, PageHeader, RelativeTime, SkeletonList, SkeletonTable, Switch, Table, Td, Th, TextLink, useToast } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { useWorkspace } from '../../app/useWorkspace';
import { useLabelMap } from '../labels/hooks';
import { useReprocess } from '../inbox/hooks';
import { useSetupWorkflow, useToggleWorkflow, useWorkflowRuns, useWorkflows } from './hooks';

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
  const notify = useToast();

  return (
    <Page>
      <PageHeader
        kicker="Built-in follow-ups"
        title="Workflows"
        description="Ready-made follow-ups that run when an email gets a certain label, for example opening a review task for a timesheet."
        actions={
          <TextLink to={`/w/${ws}/automations`} className="text-sm font-semibold">
            Build your own in Automations
          </TextLink>
        }
      />
      {workflows.isLoading && <SkeletonList rows={2} label="Loading workflows" />}
      <ErrorNote error={workflows.error ?? toggle.error ?? reprocess.error ?? setup.error} />
      <div className="grid auto-rows-fr gap-4 md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-4">
        {workflows.data?.items.map((w, i) => (
          <Card key={w.id} className="rise flex h-full flex-col gap-3" style={riseStyle(i + 1)}>
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
                    <Button
                      size="sm"
                      loading={setup.isPending && setup.variables === w.id}
                      onClick={() => setup.mutate(w.id, { onSuccess: () => notify('success', `${w.name} is on.`) })}
                    >
                      Add “{setupLabelName(w)}” label &amp; enable
                    </Button>
                    <p className="text-ink-muted">Creates the label the AI uses to spot these emails, then turns the workflow on.</p>
                  </div>
                ) : (
                  <p className="mt-1 text-ink-muted">Ask a workspace admin to add the label for this workflow.</p>
                ))}
            </div>
            <div className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-rule pt-3">
              <div className="flex items-center gap-2 text-sm">
                <Switch
                  checked={w.enabled}
                  label={`${w.name} enabled`}
                  disabled={!isAdmin || !w.available || (toggle.isPending && toggle.variables?.id === w.id)}
                  onChange={(enabled) => toggle.mutate({ id: w.id, enabled })}
                />
                <span aria-hidden="true" className="text-ink-muted">{w.enabled ? 'On' : 'Off'}</span>
              </div>
              {w.last_run ? (
                <span className="flex items-center gap-1 text-xs text-ink-muted" title={`Last run: ${statusLabel(w.last_run.status)}`}>
                  <StatusBadge status={w.last_run.status} />{' '}
                  <Mono>
                    <RelativeTime iso={w.last_run.created_at} />
                  </Mono>
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
          Recent activity
        </h2>
        {runs.isLoading && <SkeletonTable rows={3} label="Loading runs" />}
        <ErrorNote error={runs.error} />
        {runs.data &&
          (runs.data.items.length === 0 ? (
            <Card>
              <EmptyState title="Nothing has run yet" description="Each time a workflow handles an email, it is listed here. Turn one on above, then wait for a matching email." />
            </Card>
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>Status</Th>
                  <Th>Message</Th>
                  <Th>Problem</Th>
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
                      <Link to={`/w/${ws}/inbox/${r.message_id}`} className="block truncate rounded-xs text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover">
                        {r.message_subject || '(no subject)'}
                      </Link>
                    </Td>
                    <Td className="max-w-xs">
                      <span className="line-clamp-2 font-mono text-xs text-danger" title={r.error ?? undefined}>
                        {r.error ?? ''}
                      </span>
                    </Td>
                    <Td className="whitespace-nowrap">
                      <Mono>
                        <RelativeTime iso={r.created_at} />
                      </Mono>
                    </Td>
                    {isAdmin && (
                      <Td className="text-right">
                        <Button
                          size="sm"
                          variant="secondary"
                          loading={reprocess.isPending && reprocess.variables === r.message_id}
                          onClick={() => reprocess.mutate(r.message_id, { onSuccess: () => notify('success', 'Queued to run again.') })}
                        >
                          Run again
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
