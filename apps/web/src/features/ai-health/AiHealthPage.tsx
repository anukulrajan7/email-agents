import { Navigate } from 'react-router';
import { Badge, Button, Card, CardTitle, EmptyState, Mono, Page, PageHeader, Spinner, Table, Td, Th, type Tone } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { RelativeTime } from '../../components/ui/RelativeTime';
import { useWorkspace } from '../../app/useWorkspace';
import { cn } from '../../lib/cn';
import { riseStyle } from '../../lib/motion';
import { budgetPercent, usd } from '../usage/format';
import { IncidentChart } from './IncidentChart';
import { KIND_CLASS, percent, seconds } from './format';
import { HEALTH_DAYS, useAiHealth, useResolveIncident } from './hooks';
import type { AiHealth, OpenIncident } from './types';

const SEVERITY_TONE: Record<string, Tone> = { high: 'danger', medium: 'warning', low: 'neutral' };

function Stat({ label, value, note, tone }: { label: string; value: string; note?: string; tone?: 'danger' }) {
  return (
    <Card>
      <p className="kicker">{label}</p>
      <p className={cn('type-display mt-1 text-3xl tracking-[-0.02em]', tone === 'danger' ? 'text-danger' : 'text-ink')}>{value}</p>
      {note && <p className="mt-1 text-sm text-ink-muted">{note}</p>}
    </Card>
  );
}

function BudgetCard({ health }: { health: AiHealth }) {
  const pct = budgetPercent(health.budget.used_usd, health.budget.daily_usd);
  return (
    <Card>
      <p className="kicker">Daily budget</p>
      <p className="type-display mt-1 text-3xl tracking-[-0.02em] text-ink">
        {usd(health.budget.used_usd)} <span className="text-lg text-ink-muted">of {usd(health.budget.daily_usd)}</span>
      </p>
      <div
        role="progressbar"
        aria-label="Today's AI budget used"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        className="mt-3 h-2 overflow-hidden rounded-xs bg-neutral-tint"
      >
        <div className={cn('h-full', pct >= 100 ? 'bg-danger' : pct >= 80 ? 'bg-review' : 'bg-success')} style={{ width: `${pct}%` }} />
      </div>
      <p className="mt-2 text-sm text-ink-muted">{pct}% used today.</p>
    </Card>
  );
}

function detailLine(details: OpenIncident['details']): string {
  if (!details) return '';
  return Object.entries(details)
    .map(([k, v]) => `${k.replace(/_/g, ' ')}: ${Array.isArray(v) ? v.join(', ') : String(v)}`)
    .join(' · ');
}

function OpenList({ incidents, onResolve, resolving }: { incidents: OpenIncident[]; onResolve: (id: string) => void; resolving: string | null }) {
  if (incidents.length === 0) {
    return (
      <Card>
        <EmptyState title="Nothing needs a look" description="When the assistant blocks a risky reply or an answer goes wrong, it is listed here until you mark it handled." />
      </Card>
    );
  }
  return (
    <ul className="flex flex-col divide-y divide-rule rounded-md border border-rule bg-paper">
      {incidents.map((i) => (
        <li key={i.id} className="flex flex-wrap items-start justify-between gap-3 p-4">
          <div className="flex min-w-0 flex-col gap-1">
            <div className="flex flex-wrap items-center gap-2">
              <Badge tone={SEVERITY_TONE[i.severity] ?? 'neutral'}>{i.severity}</Badge>
              <span className="font-semibold text-ink">{i.label}</span>
              {i.agent_name && <span className="text-sm text-ink-muted">on {i.agent_name}</span>}
              <RelativeTime iso={new Date(i.created_at).toISOString()} className="text-sm text-ink-muted" />
            </div>
            {detailLine(i.details) && <p className="font-mono text-xs break-words text-ink-muted">{detailLine(i.details)}</p>}
          </div>
          <Button variant="secondary" disabled={resolving === i.id} onClick={() => onResolve(i.id)} aria-label={`Mark handled: ${i.label}`}>
            Mark handled
          </Button>
        </li>
      ))}
    </ul>
  );
}

function TaskTable({ health }: { health: AiHealth }) {
  if (health.tasks.length === 0) {
    return (
      <Card>
        <EmptyState title="No AI work this week" description="Speed and error rates appear once the assistant has handled some mail." />
      </Card>
    );
  }
  return (
    <Table>
      <thead>
        <tr>
          <Th>Work</Th>
          <Th className="text-right">Calls</Th>
          <Th className="text-right">Typical speed</Th>
          <Th className="text-right">Slowest 5%</Th>
          <Th className="text-right">Failed</Th>
          <Th className="text-right">Declined</Th>
          <Th className="text-right">Unusable</Th>
        </tr>
      </thead>
      <tbody>
        {health.tasks.map((t) => (
          <tr key={t.task}>
            <Td className="font-semibold">{t.label}</Td>
            <Td className="text-right"><Mono>{t.calls}</Mono></Td>
            <Td className="text-right"><Mono>{seconds(t.p50_ms)}</Mono></Td>
            <Td className="text-right"><Mono>{seconds(t.p95_ms)}</Mono></Td>
            <Td className="text-right"><Mono>{percent(t.error_rate)}</Mono></Td>
            <Td className="text-right"><Mono>{percent(t.refusal_rate)}</Mono></Td>
            <Td className="text-right"><Mono>{percent(t.schema_failure_rate)}</Mono></Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}

function KindTable({ health }: { health: AiHealth }) {
  return (
    <Table>
      <thead>
        <tr>
          <Th>Kind</Th>
          <Th className="text-right">Last {HEALTH_DAYS} days</Th>
          <Th className="text-right">Still open</Th>
        </tr>
      </thead>
      <tbody>
        {health.by_kind.map((k) => (
          <tr key={k.kind}>
            <Td className="font-semibold">
              <span className="inline-flex items-center gap-2">
                <svg width="10" height="10" aria-hidden="true">
                  <rect width="10" height="10" className={KIND_CLASS[k.kind] ?? 'fill-accent'} />
                </svg>
                {k.label}
              </span>
            </Td>
            <Td className="text-right"><Mono>{k.total}</Mono></Td>
            <Td className="text-right"><Mono>{k.open}</Mono></Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}

function ShadowCard({ health }: { health: AiHealth }) {
  const s = health.shadow;
  return (
    <Card>
      <div className="flex flex-wrap items-center gap-3">
        <CardTitle>Reading mail in one step</CardTitle>
        <Badge tone={s.ready_to_switch ? 'success' : 'neutral'}>{s.ready_to_switch ? 'Ready to switch' : 'Still comparing'}</Badge>
      </div>
      <p className="mt-2 text-sm text-ink-muted">
        The newer one-step reading runs quietly beside today's. It can take over once it agrees on at least {Math.round(s.min_agreement * 100)}% of labels across {s.min_samples} emails.
      </p>
      <dl className="mt-3 grid grid-cols-2 gap-3 md:grid-cols-4">
        {[
          ['Emails compared', String(s.compared)],
          ['Labels agree', percent(s.label_agreement)],
          ['Needs-reply agree', percent(s.needs_reply_agreement)],
          ['Priority agree', percent(s.priority_agreement)],
        ].map(([term, value]) => (
          <div key={term}>
            <dt className="kicker">{term}</dt>
            <dd className="font-mono text-lg text-ink">{value}</dd>
          </div>
        ))}
      </dl>
    </Card>
  );
}

export function AiHealthPage() {
  const { ws, workspace, isAdmin } = useWorkspace();
  const health = useAiHealth(ws, isAdmin);
  const resolve = useResolveIncident(ws);

  if (workspace && !isAdmin) return <Navigate to={`/w/${ws}/inbox`} replace />;

  const data = health.data;
  return (
    <Page className="gap-6">
      <PageHeader
        kicker="Admin"
        title="AI health"
        description="What went wrong or nearly did, how fast and reliable the assistant is, and what it costs per email."
      />
      {health.isLoading && <Spinner />}
      <ErrorNote error={health.error} />
      <ErrorNote error={resolve.error} />
      {data && (
        <>
          <div className="rise grid gap-4 md:grid-cols-4" style={riseStyle(1)}>
            <Stat
              label="Open incidents"
              value={String(data.open_total)}
              note={data.open_total === 0 ? 'All clear' : 'Waiting for someone to look'}
              tone={data.open_total > 0 ? 'danger' : undefined}
            />
            <Stat label="Cost per email today" value={usd(data.cost.per_email_today_usd)} note={`${usd(data.cost.per_email_7d_usd)} over 7 days`} />
            <Stat
              label="Spend today"
              value={usd(data.cost.today_usd)}
              note={data.cost.spike ? 'More than twice the weekly average' : `${usd(data.cost.avg_daily_7d_usd)} a day on average`}
              tone={data.cost.spike ? 'danger' : undefined}
            />
            <BudgetCard health={data} />
          </div>
          <section className="rise flex flex-col gap-3" style={riseStyle(2)}>
            <CardTitle>Needs a look</CardTitle>
            <OpenList incidents={data.open} resolving={resolve.isPending ? (resolve.variables ?? null) : null} onResolve={(id) => resolve.mutate(id)} />
          </section>
          <section className="rise flex flex-col gap-3" style={riseStyle(3)}>
            <CardTitle>Incidents over time</CardTitle>
            <Card>
              <IncidentChart days={data.by_day} kinds={data.by_kind.map((k) => k.kind)} />
            </Card>
            <KindTable health={data} />
          </section>
          <section className="rise flex flex-col gap-3" style={riseStyle(4)}>
            <CardTitle>Speed and reliability, last 7 days</CardTitle>
            <TaskTable health={data} />
          </section>
          <section className="rise" style={riseStyle(5)}>
            <ShadowCard health={data} />
          </section>
        </>
      )}
    </Page>
  );
}
