import { useState } from 'react';
import { useParams } from 'react-router';
import { Button, Card, ConfirmDialog, Page, PageHeader, Skeleton, SkeletonList, TextLink, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { CaseStatusChips } from './CaseStatusChips';
import { useCancelCase, useCase } from './hooks';
import { KIND_LABEL } from './icons';
import { formatCents, isFinished, wakeText } from './labels';
import { Timeline } from './Timeline';
import type { CaseDetail } from './types';

export function BudgetBar({ spent, budget }: { spent: number; budget: number }) {
  const pct = budget > 0 ? Math.min(100, Math.round((spent / budget) * 100)) : 0;
  const text = `${formatCents(spent)} of ${formatCents(budget)} budget`;
  return (
    <div className="flex flex-col gap-1">
      <div
        role="progressbar"
        aria-label="Budget used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        aria-valuetext={text}
        className="h-2 w-full overflow-hidden rounded-xs bg-rule"
      >
        <div className={pct >= 90 ? 'h-full bg-danger' : 'h-full bg-info'} style={{ width: `${pct}%` }} />
      </div>
      <p className="font-mono text-xs text-ink-muted">{text}</p>
    </div>
  );
}

function CancelCase({ ws, id }: { ws: string; id: string }) {
  const cancel = useCancelCase(ws);
  const notify = useToast();
  const [confirming, setConfirming] = useState(false);

  return (
    <>
      <Button variant="danger" size="sm" onClick={() => setConfirming(true)}>
        Cancel case
      </Button>
      {confirming && (
        <ConfirmDialog
          title="Stop this case?"
          confirmLabel="Yes, cancel case"
          cancelLabel="Keep going"
          busy={cancel.isPending}
          error={cancel.error}
          onCancel={() => {
            cancel.reset();
            setConfirming(false);
          }}
          onConfirm={() =>
            cancel.mutate(id, {
              onSuccess: () => {
                setConfirming(false);
                notify('success', 'Case cancelled.');
              },
            })
          }
        >
          The agent will not do anything more, and nothing waiting for approval will be sent.
        </ConfirmDialog>
      )}
    </>
  );
}

function BackToAgents({ ws }: { ws: string }) {
  return (
    <TextLink to={`/w/${ws}/agents`} className="-mb-2 self-start text-sm">
      ← All cases
    </TextLink>
  );
}

function Detail({ ws, c, isAdmin }: { ws: string; c: CaseDetail; isAdmin: boolean }) {
  return (
    <Page className="gap-8">
      <BackToAgents ws={ws} />
      <PageHeader
        kicker={`${KIND_LABEL[c.kind]} case`}
        title={c.title}
        description={c.goal}
        actions={isAdmin && !isFinished(c.status) ? <CancelCase ws={ws} id={c.id} /> : undefined}
      />
      <Card className="rise flex flex-col gap-3" style={riseStyle(1)}>
        <div className="flex flex-wrap items-center justify-between gap-2">
          <CaseStatusChips c={c} />
          <p className="font-mono text-xs text-ink-muted">
            {c.steps.length} of {c.max_steps} steps
            {c.status === 'waiting_event' && c.next_wake_at ? ` · checks again ${wakeText(c.next_wake_at)}` : ''}
          </p>
        </div>
        <BudgetBar spent={c.spent_usd} budget={c.budget_usd} />
        <p className="flex flex-wrap gap-4 text-sm">
          {c.message_id && (
            <TextLink to={`/w/${ws}/inbox/${c.message_id}`}>Source email</TextLink>
          )}
          {c.requirement_id && (
            <TextLink to={`/w/${ws}/matching?req=${c.requirement_id}`}>See matches</TextLink>
          )}
        </p>
      </Card>
      <section aria-labelledby="timeline-heading" className="rise flex flex-col gap-4" style={riseStyle(2)}>
        <h2 id="timeline-heading" className="type-display text-2xl tracking-[-0.01em]">
          What the agent did
        </h2>
        <Timeline ws={ws} steps={c.steps} approvals={c.approvals} isAdmin={isAdmin} />
      </section>
    </Page>
  );
}

export function CaseDetailPage() {
  const { ws, isAdmin } = useWorkspace();
  const { caseId = '' } = useParams<{ caseId: string }>();
  const query = useCase(ws, caseId);

  return (
    <>
      {query.isLoading && (
        <Page>
          <div className="flex flex-col gap-3 border-b border-rule-strong pb-6">
            <Skeleton className="h-3 w-32" />
            <Skeleton className="h-9 w-2/3" />
          </div>
          <SkeletonList rows={3} label="Loading case" />
        </Page>
      )}
      {query.error && (
        <Page>
          <BackToAgents ws={ws} />
          <ErrorNote error={query.error} />
        </Page>
      )}
      {query.data && <Detail ws={ws} c={query.data} isAdmin={isAdmin} />}
    </>
  );
}
