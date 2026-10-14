import { useState } from 'react';
import { Card, EmptyState, Page, PageHeader, SkeletonList, Tabs, type TabItem } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { HelpPopover } from '../help/HelpPopover';
import { LeadAgentCard, MyProfileCard, TeamProfiles } from './AgentSettings';
import { ApprovalCard } from './ApprovalCard';
import { CaseRow } from './CaseRow';
import { useApprovals, useCases } from './hooks';
import { FILTERS, isWaiting } from './labels';
import type { CaseFilter } from './types';

const EMPTY_CASES: Record<CaseFilter, string> = {
  all: 'No cases yet. Open an email and choose “Let the agent handle it”, or start the agent on a requirement.',
  working: 'Nothing is being worked on right now.',
  waiting: 'No cases are waiting.',
  done: 'No finished cases yet.',
  failed: 'Nothing has failed.',
};

function WaitingForYou({ ws }: { ws: string }) {
  const approvals = useApprovals(ws);
  const items = approvals.data?.items ?? [];
  return (
    <section aria-labelledby="waiting-heading" className="rise flex flex-col gap-3" style={riseStyle(1)}>
      <h2 id="waiting-heading" className="type-display text-2xl tracking-[-0.01em]">
        Waiting for you
        {items.length > 0 && <span className="ml-3 rounded-full bg-accent px-2 py-0.5 align-middle font-mono text-xs text-accent-ink">{items.length}</span>}
      </h2>
      {approvals.isLoading && <SkeletonList rows={1} label="Loading approvals" />}
      <ErrorNote error={approvals.error} />
      {approvals.data &&
        (items.length === 0 ? (
          <Card className="py-2">
            <EmptyState title="You are all caught up" description="When an agent needs your OK before sending something, it shows up here." />
          </Card>
        ) : (
          <ul className="flex flex-col gap-4">
            {items.map((a) => (
              <li key={a.id}>
                <ApprovalCard ws={ws} approval={a} />
              </li>
            ))}
          </ul>
        ))}
    </section>
  );
}

function CasesList({ ws, filter }: { ws: string; filter: CaseFilter }) {
  const cases = useCases(ws, filter);
  const items = (cases.data?.items ?? []).filter((c) => filter !== 'waiting' || isWaiting(c));
  return (
    <div className="flex flex-col gap-3">
      {cases.isLoading && <SkeletonList rows={3} label="Loading cases" />}
      <ErrorNote error={cases.error} />
      {cases.data &&
        (items.length === 0 ? (
          <p className="rounded-sm border border-dashed border-rule-strong p-6 text-sm text-ink-muted">{EMPTY_CASES[filter]}</p>
        ) : (
          <ul className="flex flex-col gap-3">
            {items.map((c) => (
              <CaseRow key={c.id} ws={ws} item={c} />
            ))}
          </ul>
        ))}
    </div>
  );
}

export function AgentsPage() {
  const { ws, isAdmin } = useWorkspace();
  const [filter, setFilter] = useState<CaseFilter>('all');
  const tabs: TabItem<CaseFilter>[] = FILTERS;

  return (
    <Page className="gap-8">
      <PageHeader
        kicker="Work that follows through"
        title="Agents"
        actions={<HelpPopover topic="approvals" label="About approvals" />}
        description="Agents take an email or a client requirement from start to finish. They always ask you before sending anything important."
      />
      <div className="rise grid grid-cols-1 items-stretch gap-4 lg:grid-cols-2" style={riseStyle(1)}>
        <LeadAgentCard ws={ws} />
        <MyProfileCard ws={ws} />
      </div>
      <WaitingForYou ws={ws} />
      <section aria-labelledby="cases-heading" className="rise flex flex-col gap-3" style={riseStyle(2)}>
        <h2 id="cases-heading" className="type-display text-2xl tracking-[-0.01em]">
          Cases
        </h2>
        <Tabs label="Case filters" tabs={tabs} value={filter} onChange={setFilter}>
          <CasesList ws={ws} filter={filter} />
        </Tabs>
      </section>
      {isAdmin && <TeamProfiles ws={ws} />}
    </Page>
  );
}
