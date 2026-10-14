import { useState } from 'react';
import { useNavigate, useSearchParams } from 'react-router';
import { useWorkspace } from '../../app/useWorkspace';
import { ErrorNote } from '../../components/ErrorNote';
import { Badge, Button, Card, CardTitle, EmptyState, Label, Page, PageHeader, Select, Spinner, Textarea } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { describeSending, describeStep, describeTrigger, formatUsd } from './describe';
import { useDesignerSession, useDryRun, useSaveDesign, useStartDesign, useTellDesigner } from './hooks';
import type { DesignerSession, DryRunReport } from './types';

const EXAMPLE = 'When a customer asks for a refund, collect the order number, draft a reply and ask me before sending';

function Describe({ ws }: { ws: string }) {
  const [, setParams] = useSearchParams();
  const [goal, setGoal] = useState('');
  const start = useStartDesign(ws);
  const submit = () => start.mutate(goal.trim(), { onSuccess: (s) => setParams({ session: s.id }, { replace: true }) });
  return (
    <Card className="rise flex flex-col gap-3" style={riseStyle(1)}>
      <Label htmlFor="agent-goal">What should this agent do?</Label>
      <Textarea
        id="agent-goal"
        rows={5}
        maxLength={2000}
        value={goal}
        placeholder={EXAMPLE}
        aria-describedby="agent-goal-hint"
        onChange={(e) => setGoal(e.target.value)}
      />
      <p id="agent-goal-hint" className="text-sm text-ink-muted">
        Say it the way you would tell a new colleague. You will see the plan in plain words and can try it on recent mail before it does anything.
      </p>
      <ErrorNote error={start.error} />
      <div className="flex flex-wrap gap-2">
        <Button onClick={submit} loading={start.isPending} disabled={goal.trim().length === 0}>
          Design the agent
        </Button>
        <Button variant="ghost" onClick={() => setGoal(EXAMPLE)}>
          Use an example
        </Button>
      </div>
    </Card>
  );
}

function List({ title, items }: { title: string; items: string[] }) {
  if (items.length === 0) return null;
  return (
    <section>
      <h3 className="kicker">{title}</h3>
      <ul className="mt-2 flex list-disc flex-col gap-1 pl-5 text-[15px] text-ink">
        {items.map((i) => (
          <li key={i}>{i}</li>
        ))}
      </ul>
    </section>
  );
}

function Plan({ session }: { session: DesignerSession }) {
  const spec = session.spec;
  if (!spec) {
    return (
      <Card>
        <EmptyState title="No plan yet" description="Tell the designer a little more about the job and it will try again." />
      </Card>
    );
  }
  return (
    <Card className="flex flex-col gap-5" aria-labelledby="plan-heading">
      <div>
        <p className="kicker">The plan</p>
        <CardTitle id="plan-heading" className="mt-1 text-2xl">
          {spec.name}
        </CardTitle>
        <p className="mt-2 max-w-[62ch] text-[15px] text-ink-muted">{spec.description}</p>
      </div>
      <List title="When it acts" items={spec.triggers.map(describeTrigger)} />
      {spec.mode === 'workflow' ? <List title="What it does, in order" items={spec.steps.map(describeStep)} /> : <List title="How it works" items={[spec.goal]} />}
      <section>
        <h3 className="kicker">Sending</h3>
        <p className="mt-2 text-[15px] text-ink">{describeSending(spec.permissions)}</p>
      </section>
      <List title="Rules it follows" items={spec.drafting.rules} />
      <List title="It will never" items={spec.drafting.never_do} />
      <List title="Assumptions I made" items={session.assumptions} />
      <List title="Questions for you" items={session.questions} />
      {session.issues.length > 0 && (
        <div role="alert" className="rounded-sm border-l-[3px] border-danger bg-danger-tint px-3 py-2 text-sm text-danger">
          <p className="font-semibold">This cannot be saved yet.</p>
          <ul className="mt-1 list-disc pl-5">
            {session.issues.map((i, n) => (
              <li key={`${i.code}-${n}`}>{i.message}</li>
            ))}
          </ul>
        </div>
      )}
    </Card>
  );
}

function Refine({ ws, session }: { ws: string; session: DesignerSession }) {
  const [text, setText] = useState('');
  const tell = useTellDesigner(ws, session.id);
  const last = [...session.turns].reverse().find((t) => t.role === 'designer');
  return (
    <Card className="flex flex-col gap-3">
      {last && <p className="text-[15px] text-ink">{last.text}</p>}
      <Label htmlFor="agent-change">Want something different?</Label>
      <Textarea id="agent-change" rows={2} value={text} onChange={(e) => setText(e.target.value)} placeholder="Make the replies shorter and sign off with my first name" />
      <ErrorNote error={tell.error} />
      <div className="flex items-center gap-3">
        <Button variant="secondary" loading={tell.isPending} disabled={text.trim().length === 0} onClick={() => tell.mutate(text.trim(), { onSuccess: () => setText('') })}>
          Change the plan
        </Button>
        <span className="text-xs text-ink-muted">
          {session.limits.calls_today} of {session.limits.daily_limit} designer requests used today
        </span>
      </div>
    </Card>
  );
}

function Report({ report }: { report: DryRunReport }) {
  if (report.status === 'running') {
    return (
      <p role="status" className="flex items-center gap-2 text-sm text-ink-muted">
        <Spinner className="h-4 w-4" /> Trying it on your recent mail. Nothing is written or sent.
      </p>
    );
  }
  if (report.status === 'failed') return <ErrorNote error={new Error(report.error ?? 'The try-out could not finish.')} />;
  const handled = report.would_handle ?? 0;
  return (
    <div className="flex flex-col gap-4" aria-live="polite">
      <p className="text-[15px] text-ink">
        It would take <strong>{handled}</strong> of the last <strong>{report.sampled ?? 0}</strong> emails. Estimated cost{' '}
        <strong>{formatUsd(report.est_cost_per_email_usd ?? 0)}</strong> per email, about <strong>{formatUsd(report.est_monthly_usd ?? 0)}</strong> a month.
      </p>
      {(report.steals ?? []).map((s) => (
        <p key={s.agent_id} className="text-sm text-ink-muted">
          {s.count} of those {s.count === 1 ? 'is' : 'are'} handled by {s.name} today.
        </p>
      ))}
      {(report.warnings ?? []).map((w) => (
        <p key={w} className="rounded-sm border-l-[3px] border-accent bg-surface px-3 py-2 text-sm text-ink">
          {w}
        </p>
      ))}
      <ul className="flex flex-col gap-3">
        {(report.examples ?? []).map((ex) => (
          <li key={ex.message_id} className="rounded-sm border border-rule p-3">
            <p className="font-semibold text-ink">{ex.subject}</p>
            <p className="text-sm text-ink-muted">{ex.route_reason}</p>
            <ol className="mt-2 list-decimal pl-5 text-sm text-ink">
              {ex.steps.map((st, n) => (
                <li key={n}>{st.summary}</li>
              ))}
            </ol>
            {ex.draft && <blockquote className="mt-2 border-l-2 border-rule-strong pl-3 text-sm whitespace-pre-wrap text-ink-muted">{ex.draft}</blockquote>}
            {ex.needs_person && <Badge tone="neutral">Asks you first</Badge>}
          </li>
        ))}
      </ul>
    </div>
  );
}

function TryAndSave({ ws, session }: { ws: string; session: DesignerSession }) {
  const navigate = useNavigate();
  const dry = useDryRun(ws, session.id);
  const save = useSaveDesign(ws, session.id);
  const [level, setLevel] = useState<'0' | '1'>('0');
  const [confirmed, setConfirmed] = useState(false);
  const valid = session.spec !== null && session.issues.length === 0;
  const report = session.dry_run;
  const tried = report?.status === 'done';
  const steals = tried ? (report.steals ?? []) : [];
  const needsConfirm = steals.length > 0 && !confirmed;
  return (
    <Card className="flex flex-col gap-4" aria-labelledby="try-heading">
      <CardTitle id="try-heading">Try it, then save it</CardTitle>
      <div>
        <Button variant="secondary" onClick={() => dry.mutate()} loading={dry.isPending || report?.status === 'running'} disabled={!valid}>
          {tried ? 'Try again on recent mail' : 'Try it on recent mail'}
        </Button>
      </div>
      <ErrorNote error={dry.error} />
      {report && <Report report={report} />}
      <div className="flex flex-col gap-2 border-t border-rule pt-4">
        <Label htmlFor="agent-level">Start as</Label>
        <Select id="agent-level" value={level} onChange={(e) => setLevel(e.target.value as '0' | '1')} className="max-w-sm">
          <option value="0">Watching only. It shows what it would do.</option>
          <option value="1">Drafting. It prepares drafts for you to approve.</option>
        </Select>
        {steals.length > 0 && (
          <label className="flex items-start gap-2 text-sm text-ink">
            <input type="checkbox" className="mt-1" checked={confirmed} onChange={(e) => setConfirmed(e.target.checked)} />
            <span>I am happy for this agent to take emails that {steals.map((s) => s.name).join(', ')} handle today.</span>
          </label>
        )}
        <ErrorNote error={save.error} />
        <div>
          <Button
            loading={save.isPending}
            disabled={!valid || !tried || needsConfirm}
            onClick={() => save.mutate({ start_level: Number(level) as 0 | 1, confirm_steals: confirmed }, { onSuccess: () => navigate(`/w/${ws}/agents`) })}
          >
            Save the agent
          </Button>
          {!tried && <span className="ml-3 text-sm text-ink-muted">Try it on recent mail first.</span>}
        </div>
      </div>
    </Card>
  );
}

function Session({ ws, id }: { ws: string; id: string }) {
  const q = useDesignerSession(ws, id);
  if (q.isLoading) return <p role="status" className="text-sm text-ink-muted">Loading the plan</p>;
  if (q.error || !q.data) return <ErrorNote error={q.error ?? new Error('That session was not found.')} />;
  const s = q.data;
  return (
    <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
      <div className="rise flex flex-col gap-4" style={riseStyle(1)}>
        <Plan session={s} />
        <Refine ws={ws} session={s} />
      </div>
      <div className="rise" style={riseStyle(2)}>
        <TryAndSave ws={ws} session={s} />
      </div>
    </div>
  );
}

export function AgentBuilderPage() {
  const { ws, isAdmin } = useWorkspace();
  const [params] = useSearchParams();
  const id = params.get('session') ?? undefined;
  return (
    <Page className="gap-8">
      <PageHeader
        kicker="Build your own"
        title="Create an agent"
        description="Describe the job in your own words. You will see the plan, try it on recent mail without anything being sent, and only then save it."
      />
      {!isAdmin ? (
        <Card>
          <EmptyState title="Only admins can create agents" description="Ask an admin of this workspace to set one up." />
        </Card>
      ) : id ? (
        <Session ws={ws} id={id} />
      ) : (
        <Describe ws={ws} />
      )}
    </Page>
  );
}
