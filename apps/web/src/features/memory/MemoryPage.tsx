import { useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { Card, EmptyState, Input, Label, Mono, Page, PageHeader, RelativeTime, SkeletonList, SkeletonTable, Table, Tabs, Td, Th, type TabItem } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { useDebounced } from '../../lib/useDebounced';
import { riseStyle } from '../../lib/motion';
import { useContacts, useMemories } from './hooks';
import { HelpPopover } from '../help/HelpPopover';
import { LabelStamp } from '../labels/LabelStamp';
import { groupMemories, matchesTab } from './grouping';
import { LearnedItem } from './LearnedItem';
import { TeachFact } from './TeachFact';
import type { LearnedMemory, MemoryTab } from './types';

const EMPTY: Record<MemoryTab, { title: string; description: string }> = {
  all: {
    title: 'Nothing learned yet',
    description:
      'The assistant learns from your emails and from the replies your team sends or approves. Facts it picks up will appear here on their own.',
  },
  auto: {
    title: 'Nothing learned automatically yet',
    description: 'As emails arrive and replies go out, the assistant picks up facts about contacts, clients and how your team writes.',
  },
  manual: {
    title: 'You have not added any facts',
    description: 'The assistant learns on its own, so you rarely need to. If you want to teach it something, use "Teach a fact" below.',
  },
};

function Learned({ ws }: { ws: string }) {
  const [tab, setTab] = useState<MemoryTab>('all');
  const active = useMemories(ws, { status: 'active' });
  const suggested = useMemories(ws, { status: 'suggested' });

  const all: LearnedMemory[] = [...(active.data?.items ?? []), ...(suggested.data?.items ?? [])];
  const tabs: TabItem<MemoryTab>[] = [
    { id: 'all', label: 'All', count: all.length },
    { id: 'auto', label: 'Learned automatically', count: all.filter((m) => matchesTab(m, 'auto')).length },
    { id: 'manual', label: 'Added by you', count: all.filter((m) => matchesTab(m, 'manual')).length },
  ];
  const groups = groupMemories(all.filter((m) => matchesTab(m, tab)));
  const loaded = active.data !== undefined;

  return (
    <section aria-label="What the assistant has learned" className="rise flex flex-col gap-5" style={riseStyle(1)}>
      <ErrorNote error={active.error ?? suggested.error} />
      {active.isLoading && <SkeletonList rows={3} label="Loading memories" />}
      {loaded && (
        <Tabs label="Filter memories" tabs={tabs} value={tab} onChange={setTab}>
          {groups.length === 0 ? (
            <Card>
              <EmptyState title={EMPTY[tab].title} description={EMPTY[tab].description} />
            </Card>
          ) : (
            groups.map((g) => (
              <section key={g.key} aria-labelledby={`group-${g.key}`} className="flex flex-col gap-3">
                <div>
                  <h2 id={`group-${g.key}`} className="kicker">
                    {g.title}
                  </h2>
                  <p className="mt-1 text-sm text-ink-muted">{g.hint}</p>
                </div>
                <ul className="flex flex-col gap-3">
                  {g.items.map((m) => (
                    <LearnedItem key={m.id} ws={ws} memory={m} />
                  ))}
                </ul>
              </section>
            ))
          )}
        </Tabs>
      )}
      <TeachFact ws={ws} />
    </section>
  );
}

function Contacts({ ws }: { ws: string }) {
  const navigate = useNavigate();
  const [search, setSearch] = useState('');
  const q = useDebounced(search.trim());
  const contacts = useContacts(ws, q);
  const open = (email: string) => navigate(`/w/${ws}/memory/contacts/${encodeURIComponent(email)}`);

  return (
    <section aria-labelledby="contacts-heading" className="rise flex flex-col gap-4" style={riseStyle(2)}>
      <div className="flex flex-wrap items-end justify-between gap-4">
        <h2 id="contacts-heading" className="kicker">
          Everyone who emails you
        </h2>
        <div className="flex w-full flex-col gap-1 sm:w-auto">
          <Label htmlFor="contact-search">Search contacts</Label>
          <Input id="contact-search" type="search" className="w-full sm:w-64" value={search} onChange={(e) => setSearch(e.target.value)} />
        </div>
      </div>
      {contacts.isLoading && <SkeletonTable rows={4} label="Loading contacts" />}
      <ErrorNote error={contacts.error} />
      {contacts.data &&
        (contacts.data.items.length === 0 ? (
          <Card>
            <EmptyState title="No contacts yet" description="Every sender who emails this workspace appears here, with the labels their mail usually gets." />
          </Card>
        ) : (
          <Table>
            <thead>
              <tr>
                <Th>Contact</Th>
                <Th>Messages</Th>
                <Th>Usual labels</Th>
                <Th>Last seen</Th>
              </tr>
            </thead>
            <tbody>
              {contacts.data.items.map((c) => (
                <tr key={c.email} className="cursor-pointer" onClick={() => open(c.email)}>
                  <Td>
                    <Link
                      to={`/w/${ws}/memory/contacts/${encodeURIComponent(c.email)}`}
                      onClick={(e) => e.stopPropagation()}
                      className="block font-semibold text-accent underline decoration-current/40 underline-offset-[3px] hover:text-accent-hover"
                    >
                      {c.display_name ?? c.email}
                    </Link>
                    {c.display_name && <Mono className="block break-all">{c.email}</Mono>}
                  </Td>
                  <Td>
                    <Mono>{c.message_count}</Mono>
                  </Td>
                  <Td>
                    <span className="flex flex-wrap gap-1">
                      {c.top_categories.map((t) => (
                        <LabelStamp key={t.category} ws={ws} category={t.category} />
                      ))}
                    </span>
                  </Td>
                  <Td className="whitespace-nowrap">
                    <Mono>
                      <RelativeTime iso={c.last_seen_at} />
                    </Mono>
                  </Td>
                </tr>
              ))}
            </tbody>
          </Table>
        ))}
    </section>
  );
}

export function MemoryPage() {
  const { ws } = useWorkspace();

  return (
    <Page>
      <PageHeader
        kicker="The ledger of what we know"
        title="What the assistant has learned"
        actions={<HelpPopover topic="memory" />}
        description="Facts the assistant picked up from your emails and replies. It uses them when it labels and drafts. Pin what matters, confirm what it is unsure about, delete what is wrong."
      />
      <Learned ws={ws} />
      <Contacts ws={ws} />
    </Page>
  );
}
