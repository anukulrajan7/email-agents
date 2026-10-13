import { Link, useParams } from 'react-router';
import { Card, CardTitle, EmptyState, Mono, Spinner, Stamp } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { fullDate, relativeTime } from '../../lib/format';
import { riseStyle } from '../../lib/motion';
import { useContact } from './hooks';
import { MemoryForm, MemoryItem } from './MemoryParts';

export function ContactPage() {
  const { ws } = useWorkspace();
  const { email = '' } = useParams<{ email: string }>();
  const { data, isLoading, error } = useContact(ws, email);

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-8 px-10 py-10">
      <Link to={`/w/${ws}/memory`} className="text-sm text-accent underline underline-offset-2 hover:text-accent-hover">
        Back to Memory
      </Link>
      {isLoading && <Spinner />}
      <ErrorNote error={error} />
      {data && (
        <>
          <header className="rise flex flex-col gap-3 border-b border-rule-strong pb-6" style={riseStyle(0)}>
            <p className="kicker">Contact</p>
            <h1 className="type-display text-4xl tracking-[-0.02em] break-words text-ink">{data.contact.display_name ?? data.contact.email}</h1>
            <p className="font-mono text-[13px] break-all">{data.contact.email}</p>
            <dl className="grid grid-cols-[auto_1fr] items-baseline gap-x-6 gap-y-1.5 text-sm">
              <dt className="kicker">Messages</dt>
              <dd className="font-mono text-[13px]">{data.contact.message_count}</dd>
              <dt className="kicker">First seen</dt>
              <dd className="font-mono text-[13px]">{fullDate(data.contact.first_seen_at)}</dd>
              <dt className="kicker">Last seen</dt>
              <dd className="font-mono text-[13px]">{fullDate(data.contact.last_seen_at)}</dd>
              <dt className="kicker">Usual categories</dt>
              <dd className="flex flex-wrap gap-1">
                {data.contact.top_categories.length === 0 ? (
                  <span className="text-ink-muted">None yet</span>
                ) : (
                  data.contact.top_categories.map((t) => <Stamp key={t.category} category={t.category} />)
                )}
              </dd>
            </dl>
          </header>

          <section aria-labelledby="contact-memories" className="rise flex flex-col gap-4" style={riseStyle(1)}>
            <h2 id="contact-memories" className="kicker">
              What we know about this sender
            </h2>
            <Card>
              <CardTitle className="mb-3">Add a note</CardTitle>
              <MemoryForm ws={ws} scope="contact" contactEmail={data.contact.email} textareaLabel="Note about this sender" />
            </Card>
            {data.memories.length === 0 ? (
              <p className="text-sm text-ink-muted">No notes yet.</p>
            ) : (
              <ul className="divide-y divide-rule border-y border-rule">
                {data.memories.map((m) => (
                  <MemoryItem key={m.id} ws={ws} memory={m} />
                ))}
              </ul>
            )}
          </section>

          <section aria-labelledby="contact-messages" className="rise flex flex-col gap-3" style={riseStyle(2)}>
            <h2 id="contact-messages" className="kicker">
              Recent messages
            </h2>
            {data.recent_messages.length === 0 ? (
              <EmptyState title="No messages" />
            ) : (
              <ul className="divide-y divide-rule border-y border-rule text-sm">
                {data.recent_messages.map((m) => (
                  <li key={m.id} className="flex items-center justify-between gap-3 py-2">
                    <span className="flex min-w-0 items-center gap-2">
                      {m.category && <Stamp category={m.category} />}
                      <Link to={`/w/${ws}/inbox/${m.id}`} className="truncate text-accent underline underline-offset-2 hover:text-accent-hover">
                        {m.subject || '(no subject)'}
                      </Link>
                    </span>
                    <Mono className="shrink-0">{relativeTime(m.received_at)}</Mono>
                  </li>
                ))}
              </ul>
            )}
          </section>
        </>
      )}
    </div>
  );
}
