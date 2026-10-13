import { useState } from 'react';
import { NavLink } from 'react-router';
import { Badge, Button, EmptyState, Input, Select, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { cn } from '../../lib/cn';
import { humanize, relativeTime } from '../../lib/format';
import { useDebounced } from '../../lib/useDebounced';
import { LabelStamp } from '../labels/LabelStamp';
import { useLabels } from '../labels/hooks';
import { useMessages } from './hooks';

export function PriorityBadge({ priority }: { priority: 'low' | 'normal' | 'high' | null }) {
  if (!priority) return null;
  return <Badge tone={priority === 'high' ? 'danger' : priority === 'low' ? 'neutral' : 'info'}>{humanize(priority)}</Badge>;
}

export function MessageList({ ws }: { ws: string }) {
  const [category, setCategory] = useState('');
  const [search, setSearch] = useState('');
  const q = useDebounced(search.trim());
  const labels = useLabels(ws, { includeArchived: true });
  const messages = useMessages(ws, { category: category || undefined, q: q || undefined });
  const items = messages.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-col gap-3 border-b border-rule-strong p-4">
        <div>
          <p className="kicker">Incoming</p>
          <h1 className="type-display text-3xl tracking-[-0.02em]">Inbox</h1>
          <p className="mt-1 text-sm text-ink-muted">Email that reached this workspace. The AI labels each one; you stay in control.</p>
        </div>
        <label htmlFor="msg-search" className="sr-only">
          Search messages
        </label>
        <Input id="msg-search" type="search" placeholder="Search subject or body" value={search} onChange={(e) => setSearch(e.target.value)} />
        <label htmlFor="msg-category" className="sr-only">
          Filter by label
        </label>
        <Select id="msg-category" value={category} onChange={(e) => setCategory(e.target.value)}>
          <option value="">All labels</option>
          {labels.data?.items.map((l) => (
            <option key={l.id} value={l.key}>
              {l.name}
              {l.archived ? ' (archived)' : ''}
            </option>
          ))}
        </Select>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
        {messages.isLoading && (
          <div className="flex justify-center p-6 text-ink-muted">
            <Spinner />
          </div>
        )}
        {messages.error && (
          <div className="p-3">
            <ErrorNote error={messages.error} />
          </div>
        )}
        {messages.data && items.length === 0 && (
          <EmptyState title={category || q ? 'No messages match' : 'No messages yet'}
            description={
              category || q
                ? 'Try a different label or search. Matching emails will appear here.'
                : 'Emails that reach this workspace will appear here, each with an AI label. Forward one to your inbound address or send a test email from Integrations.'
            }
          />
        )}
        <ul>
          {items.map((m) => (
            <li key={m.id} className="border-b border-rule">
              <NavLink
                to={`/w/${ws}/inbox/${m.id}`}
                className={({ isActive }) =>
                  cn(
                    'relative block px-4 py-3 motion-safe:transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
                    'before:absolute before:inset-y-0 before:left-0 before:w-[3px]',
                    isActive ? 'bg-raised before:bg-accent' : 'before:bg-transparent hover:bg-ink/[0.04]',
                  )
                }
              >
                <p className="truncate text-sm font-semibold text-ink">{m.subject || '(no subject)'}</p>
                <div className="mt-0.5 flex items-baseline justify-between gap-2 font-mono text-xs text-ink-muted">
                  <p className="min-w-0 truncate">{m.from}</p>
                  <time dateTime={m.received_at} className="shrink-0">
                    {relativeTime(m.received_at)}
                  </time>
                </div>
                <div className="mt-2 flex flex-wrap items-center gap-1">
                  {m.category && <LabelStamp ws={ws} category={m.category} />}
                  <PriorityBadge priority={m.priority} />
                  <StatusBadge status={m.processing_status} />
                  {m.needs_review && <Badge tone="warning">Needs review</Badge>}
                  {m.attachment_count > 0 && (
                    <Badge>
                      {m.attachment_count} attachment{m.attachment_count === 1 ? '' : 's'}
                    </Badge>
                  )}
                </div>
              </NavLink>
            </li>
          ))}
        </ul>
        {messages.hasNextPage && (
          <div className="p-3">
            <Button variant="secondary" className="w-full" loading={messages.isFetchingNextPage} onClick={() => void messages.fetchNextPage()}>
              Load more
            </Button>
          </div>
        )}
      </div>
    </div>
  );
}
