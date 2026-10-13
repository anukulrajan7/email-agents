import { useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import type { Candidate, Requirement } from '../../api/types';
import { ErrorNote } from '../../components/ErrorNote';
import { Badge, Button, Card, EmptyState, Mono, Page, PageHeader, Spinner, Table, Td, Th } from '../../components/ui';
import { useWorkspace } from '../../app/useWorkspace';
import { cn } from '../../lib/cn';
import { riseStyle } from '../../lib/motion';
import { StartAgentButton } from '../agents/StartAgentButton';
import { useCandidates, useCloseRequirement, useMatches, useRequirements, useSetupMatching } from './hooks';
import { experienceBand, MatchCard, noticeText, ScoreBadge, SkillChips, SourceEmailLink, SourceNote } from './MatchingParts';

const DESCRIPTION = 'Client requirements and referred candidates found in your email. The AI suggests who fits and why — you decide.';

function SetupEmptyState({ ws, isAdmin }: { ws: string; isAdmin: boolean }) {
  const setup = useSetupMatching(ws);
  const steps = [
    'Requirement emails from clients → labelled “Client requirement”.',
    'Referral emails / CVs → labelled “Candidate referral”.',
    'Matches appear here with reasons.',
  ];
  return (
    <Card className="rise mx-auto flex w-full max-w-xl flex-col items-center gap-6 py-10 text-center" style={riseStyle(1)}>
      <EmptyState title="Nothing to match yet" description="Matching connects the people you are referred with the roles clients ask for." />
      <ol className="flex flex-col gap-3 text-left text-sm">
        {steps.map((s, i) => (
          <li key={s} className="flex items-start gap-3">
            <span aria-hidden="true" className="type-display flex h-6 w-6 shrink-0 items-center justify-center rounded-full border border-rule-strong text-sm">
              {i + 1}
            </span>
            <span>{s}</span>
          </li>
        ))}
      </ol>
      {setup.isSuccess ? (
        <p role="status" className="rounded-sm border-l-[3px] border-success bg-success-tint px-3 py-2 text-sm text-success">
          Labels ready — new requirement and referral emails will be picked up automatically.
        </p>
      ) : isAdmin ? (
        <Button onClick={() => setup.mutate()} loading={setup.isPending}>
          Set up matching labels
        </Button>
      ) : (
        <p className="text-sm text-ink-muted">Ask a workspace admin to set up the matching labels.</p>
      )}
      <ErrorNote error={setup.error} />
      <Link to={`/w/${ws}/inbox`} className="text-sm text-accent underline underline-offset-2 hover:text-accent-hover">
        Go to Inbox
      </Link>
    </Card>
  );
}

function RequirementItem({ ws, req, selected, onSelect }: { ws: string; req: Requirement; selected: boolean; onSelect: () => void }) {
  const close = useCloseRequirement(ws);
  const band = experienceBand(req.min_experience, req.max_experience);
  const meta = [req.client, req.location, band].filter((x): x is string => !!x);
  return (
    <li>
      <div className={cn('rounded-sm border bg-surface', selected ? 'border-accent shadow-[inset_3px_0_0_var(--color-accent)]' : 'border-rule')}>
        <button
          type="button"
          aria-pressed={selected}
          onClick={onSelect}
          className="flex w-full flex-col gap-2 rounded-sm p-4 text-left hover:bg-ink/5 focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-accent"
        >
          <span className="type-display text-xl tracking-[-0.01em]">{req.role_title}</span>
          {meta.length > 0 && <span className="text-sm text-ink-muted">{meta.join(' · ')}</span>}
          <SkillChips skills={req.must_have_skills} label="Must-have skills" />
          <span className="flex items-center gap-2">
            <Badge tone={req.match_count > 0 ? 'info' : 'neutral'}>
              {req.match_count} {req.match_count === 1 ? 'match' : 'matches'}
            </Badge>
            {req.top_score !== null && <ScoreBadge score={req.top_score} />}
          </span>
        </button>
        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-rule px-4 py-2">
          <span className="flex flex-col gap-0.5">
            <SourceEmailLink ws={ws} messageId={req.message_id} />
            <SourceNote source={req.source} />
          </span>
          <div className="flex items-center gap-2">
            <StartAgentButton
              ws={ws}
              target={{ kind: 'requirement', requirement_id: req.id }}
              label="Start agent"
              ariaLabel={`Start agent for ${req.role_title}`}
            />
            <Button
              size="sm"
              variant="ghost"
              loading={close.isPending}
              onClick={() => close.mutate(req.id)}
              aria-label={`Close requirement ${req.role_title}`}
            >
              Close
            </Button>
          </div>
        </div>
      </div>
      <ErrorNote error={close.error} />
    </li>
  );
}

function MatchesPanel({ ws, selected }: { ws: string; selected: Requirement | undefined }) {
  const matches = useMatches(ws, selected ? { requirement_id: selected.id } : {});
  const items = matches.data?.items ?? [];
  return (
    <section aria-labelledby="matches-heading" className="flex min-w-0 flex-col gap-3">
      <h2 id="matches-heading" className="kicker">
        {selected ? `Matches for ${selected.role_title}` : 'Top matches'}
      </h2>
      {matches.isLoading ? (
        <div className="flex justify-center p-8 text-ink-muted">
          <Spinner />
        </div>
      ) : matches.error ? (
        <ErrorNote error={matches.error} />
      ) : items.length === 0 ? (
        <p className="rounded-sm border border-dashed border-rule-strong p-6 text-sm text-ink-muted">
          No matches yet. New candidates are scored against this requirement automatically; only scores of 40 or more are kept.
        </p>
      ) : (
        <div className="flex flex-col gap-3">
          {items.map((m) => (
            <MatchCard key={m.id} ws={ws} match={m} showRequirement={!selected} />
          ))}
        </div>
      )}
    </section>
  );
}

function CandidatesTable({ ws, items }: { ws: string; items: Candidate[] }) {
  if (items.length === 0) return <p className="rounded-sm border border-dashed border-rule-strong p-6 text-sm text-ink-muted">No candidates yet.</p>;
  return (
    <Table>
      <caption className="sr-only">Referred candidates</caption>
      <thead>
        <tr>
          <Th>Candidate</Th>
          <Th>Skills</Th>
          <Th>Experience</Th>
          <Th>Location</Th>
          <Th>Notice</Th>
          <Th>Referred by</Th>
          <Th>Matches</Th>
          <Th>Source</Th>
        </tr>
      </thead>
      <tbody>
        {items.map((c) => (
          <tr key={c.id}>
            <Td>
              <span className="font-semibold">{c.name}</span>
              <SourceNote source={c.source} />
            </Td>
            <Td>
              <SkillChips skills={c.skills} label={`Skills of ${c.name}`} />
            </Td>
            <Td>{c.total_experience !== null ? `${c.total_experience} yrs` : '—'}</Td>
            <Td>{c.location ?? '—'}</Td>
            <Td>{noticeText(c.notice_days) ?? '—'}</Td>
            <Td>
              <Mono>{c.referred_by ?? '—'}</Mono>
            </Td>
            <Td>{c.match_count}</Td>
            <Td>
              <SourceEmailLink ws={ws} messageId={c.message_id} />
            </Td>
          </tr>
        ))}
      </tbody>
    </Table>
  );
}

type Tab = 'requirements' | 'candidates';

export function MatchingPage() {
  const { ws, isAdmin } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const [tab, setTab] = useState<Tab>('requirements');
  const requirements = useRequirements(ws);
  const candidates = useCandidates(ws);

  const reqItems = requirements.data?.items ?? [];
  const candItems = candidates.data?.items ?? [];
  const open = reqItems.filter((r) => r.status === 'open');
  const reqId = params.get('req') ?? undefined;
  const selected = open.find((r) => r.id === reqId);

  const select = (id: string | undefined) =>
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (id && id !== prev.get('req')) next.set('req', id);
        else next.delete('req');
        return next;
      },
      { replace: true },
    );

  const header = <PageHeader kicker="Referrals × requirements" title="Matching" description={DESCRIPTION} />;

  if (requirements.isLoading || candidates.isLoading) {
    return (
      <Page>
        {header}
        <div className="flex justify-center p-12 text-ink-muted">
          <Spinner />
        </div>
      </Page>
    );
  }

  const loadError = requirements.error ?? candidates.error;
  if (loadError) {
    return (
      <Page>
        {header}
        <ErrorNote error={loadError} />
      </Page>
    );
  }

  if (reqItems.length === 0 && candItems.length === 0) {
    return (
      <Page>
        {header}
        <SetupEmptyState ws={ws} isAdmin={isAdmin} />
      </Page>
    );
  }

  const tabs: { key: Tab; label: string }[] = [
    { key: 'requirements', label: `Requirements (${open.length})` },
    { key: 'candidates', label: `Candidates (${candItems.length})` },
  ];

  return (
    <Page>
      {header}
      <div role="tablist" aria-label="Matching views" className="rise -mb-4 flex gap-1 border-b border-rule" style={riseStyle(1)}>
        {tabs.map((t) => (
          <button
            key={t.key}
            type="button"
            role="tab"
            id={`tab-${t.key}`}
            aria-selected={tab === t.key}
            aria-controls={`panel-${t.key}`}
            onClick={() => setTab(t.key)}
            className={cn(
              '-mb-px border-b-2 px-4 py-2 text-[15px] font-semibold focus-visible:outline-2 focus-visible:outline-accent',
              tab === t.key ? 'border-accent text-ink' : 'border-transparent text-ink-muted hover:text-ink',
            )}
          >
            {t.label}
          </button>
        ))}
      </div>
      {tab === 'requirements' ? (
        <div
          role="tabpanel"
          id="panel-requirements"
          aria-labelledby="tab-requirements"
          className="rise grid items-start gap-8 xl:grid-cols-[minmax(0,2fr)_minmax(0,3fr)]"
          style={riseStyle(2)}
        >
          <section aria-labelledby="open-reqs-heading" className="flex min-w-0 flex-col gap-3">
            <h2 id="open-reqs-heading" className="kicker">
              Open requirements
            </h2>
            {open.length === 0 ? (
              <p className="rounded-sm border border-dashed border-rule-strong p-6 text-sm text-ink-muted">No open requirements.</p>
            ) : (
              <ul className="flex flex-col gap-3">
                {open.map((r) => (
                  <RequirementItem key={r.id} ws={ws} req={r} selected={r.id === selected?.id} onSelect={() => select(r.id)} />
                ))}
              </ul>
            )}
            {selected && (
              <Button size="sm" variant="ghost" className="self-start" onClick={() => select(undefined)}>
                Show all top matches
              </Button>
            )}
          </section>
          <MatchesPanel ws={ws} selected={selected} />
        </div>
      ) : (
        <div role="tabpanel" id="panel-candidates" aria-labelledby="tab-candidates" className="rise" style={riseStyle(2)}>
          <CandidatesTable ws={ws} items={candItems} />
        </div>
      )}
    </Page>
  );
}
