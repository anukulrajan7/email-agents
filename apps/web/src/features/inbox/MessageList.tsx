import { useState } from 'react';
import { NavLink } from 'react-router';
import { Badge, Button, EmptyState, Input, Select, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { relativeTime } from '../../lib/format';
import { useDebounced } from '../../lib/useDebounced';
import { LabelStamp } from '../labels/LabelStamp';
import { useLabels } from '../labels/hooks';
import type { MessageSummary } from '../../api/types';
import { parseAddress } from './emailText';
import { useMessages, type MessageSort, type ReplyFilter } from './hooks';

const SORT_KEY = 'inbox.sort';

function readSort(): MessageSort {
  try {
    return localStorage.getItem(SORT_KEY) === 'recent' ? 'recent' : 'importance';
  } catch {
    return 'importance';
  }
}

function writeSort(sort: MessageSort): void {
  try {
    localStorage.setItem(SORT_KEY, sort);
  } catch {
    return undefined;
  }
}

function SortToggle({ sort, onChange }: { sort: MessageSort; onChange: (sort: MessageSort) => void }) {
  const options: { value: MessageSort; label: string }[] = [
    { value: 'importance', label: 'Important first' },
    { value: 'recent', label: 'Newest' },
  ];
  return (
    <div role="group" aria-label="Sort messages" className="inline-flex divide-x divide-rule overflow-hidden rounded-sm border border-rule-strong self-start">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          aria-pressed={sort === o.value}
          onClick={() => onChange(o.value)}
          className={cn(
            'px-3 py-1 text-[13px] font-semibold transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
            sort === o.value ? 'bg-ink text-accent-ink' : 'bg-raised text-ink-muted hover:text-ink',
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

function Row({ ws, m }: { ws: string; m: MessageSummary }) {
  const sender = parseAddress(m.from).name;
  const unread = m.unread === true;
  const summary = m.summary?.trim();
  return (
    <NavLink
      to={`/w/${ws}/inbox/${m.id}`}
      className={({ isActive }) =>
        cn(
          'relative block px-4 py-2 motion-safe:transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
          'before:absolute before:inset-y-0 before:left-0 before:w-[3px]',
          isActive ? 'bg-raised before:bg-accent' : 'before:bg-transparent hover:bg-ink/[0.04]',
        )
      }
    >
      <div className="flex items-baseline justify-between gap-2">
        <p className={cn('flex min-w-0 items-center gap-1.5 text-sm text-ink', unread ? 'font-bold' : 'font-medium')}>
          {m.priority === 'high' && (
            <span role="img" aria-label="High priority" title="High priority" className="h-2 w-2 shrink-0 rounded-full bg-accent" />
          )}
          <span className="truncate">{sender}</span>
        </p>
        <time dateTime={m.received_at} className="shrink-0 text-xs text-ink-muted">
          {relativeTime(m.received_at)}
        </time>
      </div>
      <p className="truncate text-sm text-ink-muted">
        <span className={cn('text-ink', unread ? 'font-bold' : 'font-normal')}>{m.subject || '(no subject)'}</span>
        {summary && <span> — {summary}</span>}
      </p>
      <div className="mt-1 flex flex-wrap items-center gap-1">
        {m.category && <LabelStamp ws={ws} category={m.category} />}
        {m.replied ? <Badge tone="success">Replied</Badge> : m.needs_reply && <Badge tone="danger">Needs reply</Badge>}
        {m.processing_status === 'failed' && <Badge tone="danger">Failed</Badge>}
        {m.attachment_count > 0 && (
          <Badge>
            {m.attachment_count} attachment{m.attachment_count === 1 ? '' : 's'}
          </Badge>
        )}
      </div>
    </NavLink>
  );
}

export function MessageList({ ws }: { ws: string }) {
  const [category, setCategory] = useState('');
  const [search, setSearch] = useState('');
  const [sort, setSort] = useState<MessageSort>(readSort);
  const q = useDebounced(search.trim());
  const labels = useLabels(ws, { includeArchived: true });
  // The filter holds a label key, or `reply:awaiting` / `reply:replied` for reply status.
  const reply = category.startsWith('reply:') ? (category.slice(6) as ReplyFilter) : undefined;
  const messages = useMessages(ws, { category: reply ? undefined : category || undefined, q: q || undefined, sort, reply });
  const items = messages.data?.pages.flatMap((p) => p.items) ?? [];

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-col gap-3 border-b border-rule-strong p-4">
        <div>
          <p className="kicker">Incoming</p>
          <h1 className="type-display text-3xl tracking-[-0.02em]">Inbox</h1>
          <p className="mt-1 text-sm text-ink-muted">Email that reached this workspace. Each one is labelled automatically.</p>
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
          <optgroup label="Reply status">
            <option value="reply:awaiting">Needs reply</option>
            <option value="reply:replied">Replied</option>
          </optgroup>
          <optgroup label="Labels">
            {labels.data?.items.map((l) => (
              <option key={l.id} value={l.key}>
                {l.name}
                {l.archived ? ' (archived)' : ''}
              </option>
            ))}
          </optgroup>
        </Select>
        <SortToggle
          sort={sort}
          onChange={(next) => {
            setSort(next);
            writeSort(next);
          }}
        />
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
              <Row ws={ws} m={m} />
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
