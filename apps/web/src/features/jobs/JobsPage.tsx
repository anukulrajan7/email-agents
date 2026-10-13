import { Navigate } from 'react-router';
import { Badge, Button, Card, EmptyState, Mono, Page, PageHeader, RelativeTime, SkeletonTable, Table, Td, Th, useToast } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { useDeadJobs, useRetryJob } from './hooks';

export function JobsPage() {
  const { ws, workspace, isAdmin } = useWorkspace();
  const jobs = useDeadJobs(ws, isAdmin);
  const retry = useRetryJob(ws);
  const notify = useToast();

  if (workspace && !isAdmin) return <Navigate to={`/w/${ws}/inbox`} replace />;

  return (
    <Page className="gap-6">
      <PageHeader kicker="Admin" title="Jobs" description="Background work that failed after several tries. Retry it once the cause is fixed." />
      {jobs.isLoading && <SkeletonTable rows={3} label="Loading jobs" />}
      <ErrorNote error={jobs.error ?? retry.error} />
      {jobs.data &&
        (jobs.data.items.length === 0 ? (
          <Card className="rise" style={riseStyle(1)}>
            <EmptyState title="Nothing has failed" description="Background work that fails after several tries appears here with a Retry button." />
          </Card>
        ) : (
          <div className="rise" style={riseStyle(1)}>
            <Table>
              <thead>
                <tr>
                  <Th>Kind</Th>
                  <Th>Attempts</Th>
                  <Th>Last error</Th>
                  <Th>Updated</Th>
                  <Th>
                    <span className="sr-only">Actions</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {jobs.data.items.map((j) => (
                  <tr key={j.id}>
                    <Td className="font-mono text-[13px] font-semibold">{j.kind}</Td>
                    <Td>
                      <Badge tone="danger">
                        {j.attempts}/{j.max_attempts}
                      </Badge>
                    </Td>
                    <Td className="max-w-xs">
                      <span className="line-clamp-2 font-mono text-xs text-danger" title={j.last_error ?? undefined}>
                        {j.last_error ?? '-'}
                      </span>
                    </Td>
                    <Td className="whitespace-nowrap">
                      <Mono>
                        <RelativeTime iso={j.updated_at} />
                      </Mono>
                    </Td>
                    <Td className="text-right">
                      <Button
                        size="sm"
                        variant="secondary"
                        loading={retry.isPending && retry.variables === j.id}
                        onClick={() => retry.mutate(j.id, { onSuccess: () => notify('success', 'Job queued to run again.') })}
                      >
                        Retry
                      </Button>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          </div>
        ))}
    </Page>
  );
}
