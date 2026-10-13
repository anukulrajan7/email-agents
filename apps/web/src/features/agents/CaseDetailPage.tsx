import { useState } from 'react';
import { Link, useParams } from 'react-router';
import { Button, Card, Page, PageHeader, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { CaseStatusChips } from './CaseStatusChips';
import { useCancelCase, useCase } from './hooks';
import { KIND_LABEL } from './icons';
import { formatCents, isFinished, wakeText } from './labels';
import { Timeline } from './Timeline';
import { ToastRegion, useToasts } from './Toasts';
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

function CancelCase({ ws, id, onCancelled }: { ws: string; id: string; onCancelled: () => void }) {
  const cancel = useCancelCase(ws);
  const [confirming, setConfirming] = useState(false);

  if (!confirming) {
    return (
      <Button variant="danger" size="sm" onClick={() => setConfirming(true)}>
        Cancel case
      </Button>
    );
  }
  return (
    <div role="alertdialog" aria-label="Confirm cancel" className="flex flex-col gap-2 rounded-sm border border-danger/40 bg-danger-tint p-3">
      <p className="text-sm">Stop this case? The agent will not do anything more, and nothing waiting for approval will be sent.</p>
      <ErrorNote error={cancel.error} />
      <div className="flex gap-2">
        <Button
          variant="danger"
          size="sm"
          loading={cancel.isPending}
          onClick={async () => {
            try {
              await cancel.mutateAsync(id);
              onCancelled();
            } catch {
              return;
            }
          }}
        >
          Yes, cancel case
        </Button>
        <Button variant="ghost" size="sm" onClick={() => setConfirming(false)} disabled={cancel.isPending}>
          Keep going
        </Button>
      </div>
    </div>
  );
}

function Detail({ ws, c, isAdmin }: { ws: string; c: CaseDetail; isAdmin: boolean }) {
  const { toasts, push, dismiss } = useToasts();
  return (
    <Page className="gap-8">
      <PageHeader
        kicker={`${KIND_LABEL[c.kind]} case`}
        title={c.title}
        description={c.goal}
        actions={isAdmin && !isFinished(c.status) ? <CancelCase ws={ws} id={c.id} onCancelled={() => push('success', 'Case cancelled.')} /> : undefined}
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
            <Link to={`/w/${ws}/inbox/${c.message_id}`} className="text-accent underline underline-offset-2 hover:text-accent-hover">
              Source email
            </Link>
          )}
          {c.requirement_id && (
            <Link to={`/w/${ws}/matching?req=${c.requirement_id}`} className="text-accent underline underline-offset-2 hover:text-accent-hover">
              See matches
            </Link>
          )}
        </p>
      </Card>
      <section aria-labelledby="timeline-heading" className="rise flex flex-col gap-4" style={riseStyle(2)}>
        <h2 id="timeline-heading" className="type-display text-2xl tracking-[-0.01em]">
          What the agent did
        </h2>
        <Timeline ws={ws} steps={c.steps} approvals={c.approvals} isAdmin={isAdmin} notify={push} />
      </section>
      <ToastRegion toasts={toasts} onDismiss={dismiss} />
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
        <div className="flex justify-center p-12 text-ink-muted">
          <Spinner />
        </div>
      )}
      {query.error && (
        <div className="p-6">
          <ErrorNote error={query.error} />
        </div>
      )}
      {query.data && <Detail ws={ws} c={query.data} isAdmin={isAdmin} />}
    </>
  );
}
