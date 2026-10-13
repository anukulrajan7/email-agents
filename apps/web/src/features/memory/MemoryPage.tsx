import { useState } from 'react';
import { Link, useNavigate } from 'react-router';
import { Button, Card, CardTitle, EmptyState, Input, Label, Mono, PageHeader, Spinner, Table, Td, Th, Page } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { useDebounced } from '../../lib/useDebounced';
import { relativeTime } from '../../lib/format';
import { riseStyle } from '../../lib/motion';
import { useContacts, useMemories, useUpdateMemory } from './hooks';
import { LabelStamp } from '../labels/LabelStamp';
import { KindBadge, MemoryForm, MemoryItem } from './MemoryParts';

function Suggestions({ ws }: { ws: string }) {
  const suggested = useMemories(ws, { status: 'suggested' });
  const update = useUpdateMemory(ws);
  const items = (suggested.data?.items ?? []).filter((m) => m.source === 'learned');

  if (items.length === 0) return <ErrorNote error={suggested.error} />;

  return (
    <section aria-labelledby="suggested-heading" className="rise flex flex-col gap-3" style={riseStyle(1)}>
      <div>
        <h2 id="suggested-heading" className="kicker">
          Suggested by the agent
        </h2>
        <p className="mt-1 text-sm text-ink-muted">Learned from how your team edits drafts. Nothing is used until you accept it.</p>
      </div>
      <ErrorNote error={suggested.error ?? update.error} />
      <ul className="flex flex-col gap-3">
        {items.map((m) => {
          const busy = update.isPending && update.variables?.id === m.id;
          return (
            <li key={m.id}>
              <Card className="flex flex-col gap-3">
                <div className="flex flex-wrap items-center gap-2">
                  <KindBadge kind={m.kind} />
                  {m.contact_email && <Mono>{m.contact_email}</Mono>}
                </div>
                <p className="text-[15px] break-words whitespace-pre-wrap">{m.content}</p>
                <div className="flex flex-wrap items-center gap-2">
                  <Button size="sm" disabled={busy} onClick={() => update.mutate({ id: m.id, status: 'active' })}>
                    Accept
                  </Button>
                  <Button size="sm" variant="secondary" disabled={busy} onClick={() => update.mutate({ id: m.id, status: 'archived' })}>
                    Dismiss
                  </Button>
                  {m.evidence_message_id && (
                    <Link
                      to={`/w/${ws}/inbox/${m.evidence_message_id}`}
                      className="text-sm text-accent underline underline-offset-2 hover:text-accent-hover"
                    >
                      Learned from
                    </Link>
                  )}
                </div>
              </Card>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function SearchResults({ ws, q }: { ws: string; q: string }) {
  const results = useMemories(ws, { q });
  return (
    <section aria-labelledby="search-heading" className="flex flex-col gap-2">
      <h2 id="search-heading" className="kicker">
        Search results
      </h2>
      {results.isLoading && <Spinner />}
      <ErrorNote error={results.error} />
      {results.data &&
        (results.data.items.length === 0 ? (
          <p className="text-sm text-ink-muted">No active memories match.</p>
        ) : (
          <ul className="divide-y divide-rule border-y border-rule">
            {results.data.items.map((m) => (
              <MemoryItem key={m.id} ws={ws} memory={m} />
            ))}
          </ul>
        ))}
    </section>
  );
}

function TeamNotes({ ws }: { ws: string }) {
  const notes = useMemories(ws, { status: 'active', scope: 'workspace' });
  return (
    <section aria-labelledby="team-heading" className="rise flex flex-col gap-4" style={riseStyle(2)}>
      <h2 id="team-heading" className="kicker">
        Team notes
      </h2>
      <Card>
        <CardTitle className="mb-3">Add a team note</CardTitle>
        <MemoryForm ws={ws} scope="workspace" textareaLabel="Note" />
      </Card>
      {notes.isLoading && <Spinner />}
      <ErrorNote error={notes.error} />
      {notes.data &&
        (notes.data.items.length === 0 ? (
          <p className="text-sm text-ink-muted">No team notes yet. Add one above, such as how you sign off or which leave policy applies.</p>
        ) : (
          <ul className="divide-y divide-rule border-y border-rule">
            {notes.data.items.map((m) => (
              <MemoryItem key={m.id} ws={ws} memory={m} />
            ))}
          </ul>
        ))}
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
    <section aria-labelledby="contacts-heading" className="rise flex flex-col gap-4" style={riseStyle(3)}>
      <div className="flex flex-wrap items-end justify-between gap-4">
        <h2 id="contacts-heading" className="kicker">
          Contacts
        </h2>
        <div className="flex flex-col gap-1">
          <Label htmlFor="contact-search">Search contacts</Label>
          <Input id="contact-search" type="search" className="w-64" value={search} onChange={(e) => setSearch(e.target.value)} />
        </div>
      </div>
      {contacts.isLoading && <Spinner />}
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
                      className="block font-semibold text-accent underline underline-offset-2 hover:text-accent-hover"
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
                    <Mono>{relativeTime(c.last_seen_at)}</Mono>
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
  const [search, setSearch] = useState('');
  const q = useDebounced(search.trim());

  return (
    <Page>
      <PageHeader
        kicker="The ledger of what we know"
        title="Memory"
        description="Notes the AI reads before it labels and drafts. Add team rules and sender details here."
        actions={
          <div className="flex flex-col gap-1">
            <Label htmlFor="memory-search">Search memories</Label>
            <Input id="memory-search" type="search" className="w-64" value={search} onChange={(e) => setSearch(e.target.value)} />
          </div>
        }
      />
      {q && <SearchResults ws={ws} q={q} />}
      <Suggestions ws={ws} />
      <TeamNotes ws={ws} />
      <Contacts ws={ws} />
    </Page>
  );
}
