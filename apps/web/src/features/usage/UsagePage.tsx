import { Navigate } from 'react-router';
import { Card, CardTitle, EmptyState, Mono, Page, PageHeader, Spinner, Table, Td, Th } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { cn } from '../../lib/cn';
import { riseStyle } from '../../lib/motion';
import { CostChart } from './CostChart';
import { budgetPercent, usd } from './format';
import { USAGE_DAYS, useAiUsage } from './hooks';
import type { AiUsage } from './types';

function Stat({ label, value, note }: { label: string; value: string; note?: string }) {
  return (
    <Card>
      <p className="kicker">{label}</p>
      <p className="type-display mt-1 text-3xl tracking-[-0.02em] text-ink">{value}</p>
      {note && <p className="mt-1 text-sm text-ink-muted">{note}</p>}
    </Card>
  );
}

function TodayCard({ usage }: { usage: AiUsage }) {
  const percent = budgetPercent(usage.today_usd, usage.budget_usd_per_day);
  const spent = percent >= 100;
  return (
    <Card>
      <p className="kicker">Today</p>
      <p className="type-display mt-1 text-3xl tracking-[-0.02em] text-ink">
        {usd(usage.today_usd)} <span className="text-lg text-ink-muted">of {usd(usage.budget_usd_per_day)}</span>
      </p>
      <div
        role="progressbar"
        aria-label="Today's AI budget used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        className="mt-3 h-2 overflow-hidden rounded-xs bg-neutral-tint"
      >
        <div className={cn('h-full', spent ? 'bg-danger' : percent >= 80 ? 'bg-review' : 'bg-success')} style={{ width: `${percent}%` }} />
      </div>
      <p className={cn('mt-2 text-sm', spent ? 'text-danger' : 'text-ink-muted')}>
        {spent
          ? 'Daily budget reached. Extra automatic reading waits; drafts and opened emails still work.'
          : `${percent}% of the daily budget used.`}
      </p>
    </Card>
  );
}

function TaskTable({ usage }: { usage: AiUsage }) {
  if (usage.by_task.length === 0) {
    return (
      <Card>
        <EmptyState title="No AI work yet" description="Once emails arrive and the assistant starts reading them, its cost shows up here." />
      </Card>
    );
  }
  return (
    <Table>
      <thead>
        <tr>
          <Th>Work</Th>
          <Th className="text-right">Calls</Th>
          <Th className="text-right">Cost</Th>
          <Th className="text-right">Share</Th>
        </tr>
      </thead>
      <tbody>
        {usage.by_task.map((t) => (
          <tr key={t.task}>
            <Td className="font-semibold">{t.task}</Td>
            <Td className="text-right">
              <Mono>{t.calls}</Mono>
            </Td>
            <Td className="text-right">
              <Mono>{usd(t.usd)}</Mono>
            </Td>
            <Td className="text-right">
              <Mono>{usage.total_usd > 0 ? `${Math.round((t.usd / usage.total_usd) * 100)}%` : '-'}</Mono>
            </Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}

export function UsagePage() {
  const { ws, workspace, isAdmin } = useWorkspace();
  const usage = useAiUsage(ws, isAdmin);

  if (workspace && !isAdmin) return <Navigate to={`/w/${ws}/inbox`} replace />;

  return (
    <Page className="gap-6">
      <PageHeader kicker="Admin" title="AI usage" description="What the assistant costs: today against the daily budget, and where the money goes." />
      {usage.isLoading && <Spinner />}
      <ErrorNote error={usage.error} />
      {usage.data && (
        <>
          <div className="rise grid gap-4 md:grid-cols-3" style={riseStyle(1)}>
            <TodayCard usage={usage.data} />
            <Stat
              label={`Last ${USAGE_DAYS} days`}
              value={usd(usage.data.total_usd)}
              note={`${usage.data.by_task.reduce((n, t) => n + t.calls, 0)} AI calls`}
            />
            <Stat label="Cost per email" value={usd(usage.data.per_email_usd)} note="Average automatic cost for each new email" />
          </div>
          <section className="rise flex flex-col gap-3" style={riseStyle(2)}>
            <CardTitle>Cost per day</CardTitle>
            <Card>
              <CostChart days={usage.data.by_day} />
            </Card>
          </section>
          <section className="rise flex flex-col gap-3" style={riseStyle(3)}>
            <CardTitle>Where it goes</CardTitle>
            <TaskTable usage={usage.data} />
          </section>
        </>
      )}
    </Page>
  );
}
