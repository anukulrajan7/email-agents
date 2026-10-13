import { useState } from 'react';
import { Card, EmptyState, Page, PageHeader, Spinner } from '../../components/ui';
import { Tabs, type TabItem } from '../../components/ui/Tabs';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { ApprovalCard, type NotifyResult } from './ApprovalCard';
import { CaseRow } from './CaseRow';
import { useApprovals, useCases } from './hooks';
import { FILTERS, isWaiting } from './labels';
import { ToastRegion, useToasts } from './Toasts';
import type { CaseFilter } from './types';

const EMPTY_CASES: Record<CaseFilter, string> = {
  all: 'No cases yet. Open an email and choose “Let the agent handle it”, or start the agent on a requirement.',
  working: 'Nothing is being worked on right now.',
  waiting: 'No cases are waiting.',
  done: 'No finished cases yet.',
  failed: 'Nothing has failed.',
};

function WaitingForYou({ ws, notify }: { ws: string; notify: NotifyResult }) {
  const approvals = useApprovals(ws);
  const items = approvals.data?.items ?? [];
  return (
    <section aria-labelledby="waiting-heading" className="rise flex flex-col gap-3" style={riseStyle(1)}>
      <h2 id="waiting-heading" className="type-display text-2xl tracking-[-0.01em]">
        Waiting for you
        {items.length > 0 && <span className="ml-3 font-mono text-sm text-ink-muted">{items.length}</span>}
      </h2>
      {approvals.isLoading && <Spinner />}
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
                <ApprovalCard ws={ws} approval={a} notify={notify} />
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
      {cases.isLoading && <Spinner />}
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
  const { ws } = useWorkspace();
  const [filter, setFilter] = useState<CaseFilter>('all');
  const { toasts, push, dismiss } = useToasts();
  const tabs: TabItem<CaseFilter>[] = FILTERS;

  return (
    <Page className="gap-8">
      <PageHeader
        kicker="Work that follows through"
        title="Agents"
        description="Agents handle emails and requirements start to finish. They ask you before sending anything important."
      />
      <WaitingForYou ws={ws} notify={push} />
      <section aria-labelledby="cases-heading" className="rise flex flex-col gap-3" style={riseStyle(2)}>
        <h2 id="cases-heading" className="type-display text-2xl tracking-[-0.01em]">
          Cases
        </h2>
        <Tabs label="Case filters" tabs={tabs} value={filter} onChange={setFilter}>
          <CasesList ws={ws} filter={filter} />
        </Tabs>
      </section>
      <ToastRegion toasts={toasts} onDismiss={dismiss} />
    </Page>
  );
}
