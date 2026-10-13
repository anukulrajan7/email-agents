import { useState } from 'react';
import { Link } from 'react-router';
import { Button, Card, EmptyState, Label, Mono, PageHeader, Select, Spinner, Table, Td, Th, Page } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { useWorkspace } from '../../app/useWorkspace';
import { humanize, relativeTime } from '../../lib/format';
import { useReviews, useSetReviewStatus, type ReviewFilter } from './hooks';

export function ReviewsPage() {
  const { ws } = useWorkspace();
  const [filter, setFilter] = useState<ReviewFilter>('open');
  const reviews = useReviews(ws, filter);
  const setStatus = useSetReviewStatus(ws);

  return (
    <Page className="gap-6">
      <PageHeader
        kicker="Human decisions"
        title="Reviews"
        description="Tasks created by workflows that need a person to check."
        actions={
          <div className="flex flex-col gap-1">
            <Label htmlFor="review-status">Status</Label>
            <Select id="review-status" value={filter} onChange={(e) => setFilter(e.target.value as ReviewFilter)} className="w-36">
              <option value="open">Open</option>
              <option value="closed">Closed</option>
              <option value="all">All</option>
            </Select>
          </div>
        }
      />
      {reviews.isLoading && <Spinner />}
      <ErrorNote error={reviews.error ?? setStatus.error} />
      {reviews.data &&
        (reviews.data.items.length === 0 ? (
          <Card className="rise" style={riseStyle(1)}>
            <EmptyState title="Nothing to review" description="When a workflow flags an email for a person to check, the task appears here. Turn workflows on from the Workflows page." />
          </Card>
        ) : (
          <div className="rise" style={riseStyle(1)}>
            <Table>
              <thead>
                <tr>
                  <Th>Item</Th>
                  <Th>Message</Th>
                  <Th>Kind</Th>
                  <Th>Opened</Th>
                  <Th>Status</Th>
                  <Th>
                    <span className="sr-only">Actions</span>
                  </Th>
                </tr>
              </thead>
              <tbody>
                {reviews.data.items.map((r) => (
                  <tr key={r.id}>
                    <Td className="max-w-xs font-semibold">
                      <span className="block truncate">{r.title}</span>
                    </Td>
                    <Td className="max-w-xs">
                      <Link to={`/w/${ws}/inbox/${r.message_id}`} className="block truncate text-accent underline underline-offset-2 hover:text-accent-hover">
                        {r.message_subject || '(no subject)'}
                      </Link>
                    </Td>
                    <Td className="whitespace-nowrap text-ink-muted">{humanize(r.kind)}</Td>
                    <Td className="whitespace-nowrap">
                      <Mono>{relativeTime(r.created_at)}</Mono>
                    </Td>
                    <Td>
                      <StatusBadge status={r.status} />
                    </Td>
                    <Td className="text-right">
                      <Button
                        variant="secondary"
                        size="sm"
                        loading={setStatus.isPending && setStatus.variables?.id === r.id}
                        onClick={() => setStatus.mutate({ id: r.id, status: r.status === 'open' ? 'closed' : 'open' })}
                      >
                        {r.status === 'open' ? 'Complete' : 'Reopen'}
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
