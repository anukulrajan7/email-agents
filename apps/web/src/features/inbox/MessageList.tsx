import { useState } from 'react';
import { NavLink } from 'react-router';
import { Badge, Button, ConfirmDialog, EmptyState, Input, RelativeTime, SegmentedControl, Select, Skeleton, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { useDebounced } from '../../lib/useDebounced';
import { LabelStamp } from '../labels/LabelStamp';
import { useLabels } from '../labels/hooks';
import type { MessageSummary } from '../../api/types';
import { parseAddress } from './emailText';
import { DELETE_NOTE, useDeleteMessages, useMessages, type MessageSort, type ReplyFilter } from './hooks';
import { deletedToast } from './DeleteEmail';

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

const SORT_OPTIONS: { value: MessageSort; label: string }[] = [
  { value: 'importance', label: 'Important first' },
  { value: 'recent', label: 'Newest' },
];

function RowBody({ ws, m, showUnreadDot }: { ws: string; m: MessageSummary; showUnreadDot: boolean }) {
  const sender = parseAddress(m.from).name;
  const unread = m.unread === true;
  const summary = m.summary?.trim();
  return (
    <>
      {unread && showUnreadDot && (
        <span className="absolute top-[1.15rem] left-2.5 h-2 w-2 rounded-full bg-info">
          <span className="sr-only">Unread</span>
        </span>
      )}
      <div className="flex items-baseline justify-between gap-2">
        <p className={cn('flex min-w-0 items-center gap-1.5 text-sm text-ink', unread ? 'font-bold' : 'font-medium')}>
          {m.priority === 'high' && (
            <span role="img" aria-label="High priority" title="High priority" className="h-2 w-2 shrink-0 rotate-45 bg-accent" />
          )}
          <span className="truncate">{sender}</span>
        </p>
        <RelativeTime iso={m.received_at} className={cn('shrink-0 text-xs', unread ? 'font-semibold text-ink' : 'text-ink-muted')} />
      </div>
      <p className="mt-0.5 line-clamp-2 text-sm leading-snug text-ink-muted">
        <span className={cn('text-ink', unread ? 'font-bold' : 'font-normal')}>{m.subject || '(no subject)'}</span>
        {summary && <span> — {summary}</span>}
      </p>
      <div className="mt-2 flex flex-wrap items-center gap-1.5 empty:hidden">
        {m.category && <LabelStamp ws={ws} category={m.category} />}
        {m.replied ? <Badge tone="success">Replied</Badge> : m.needs_reply && <Badge tone="danger">Needs reply</Badge>}
        {m.processing_status === 'failed' && <Badge tone="danger">Couldn&apos;t read</Badge>}
        {m.attachment_count > 0 && (
          <Badge>
            {m.attachment_count} attachment{m.attachment_count === 1 ? '' : 's'}
          </Badge>
        )}
      </div>
    </>
  );
}

function Row({ ws, m }: { ws: string; m: MessageSummary }) {
  return (
    <NavLink
      to={`/w/${ws}/inbox/${m.id}`}
      className={({ isActive }) =>
        cn(
          'group relative block py-3 pr-4 pl-6 motion-safe:transition-colors focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
          'before:absolute before:inset-y-0 before:left-0 before:w-[3px]',
          isActive ? 'bg-raised shadow-[inset_0_-1px_0_var(--color-rule),inset_0_1px_0_var(--color-rule)] before:bg-accent' : 'before:bg-transparent hover:bg-ink/[0.035]',
        )
      }
    >
      <RowBody ws={ws} m={m} showUnreadDot />
    </NavLink>
  );
}

function SelectableRow({ ws, m, checked, onToggle }: { ws: string; m: MessageSummary; checked: boolean; onToggle: () => void }) {
  return (
    <label className={cn('flex cursor-pointer items-start gap-3 py-3 pr-4 pl-4 motion-safe:transition-colors', checked ? 'bg-raised' : 'hover:bg-ink/[0.035]')}>
      <input
        type="checkbox"
        checked={checked}
        onChange={onToggle}
        aria-label={`Select ${m.subject || '(no subject)'}`}
        className="mt-1 h-4 w-4 shrink-0 accent-accent focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
      />
      <div className="relative min-w-0 flex-1">
        <RowBody ws={ws} m={m} showUnreadDot={false} />
      </div>
    </label>
  );
}

function ListSkeleton() {
  return (
    <div role="status" aria-label="Loading messages" aria-busy="true">
      {Array.from({ length: 6 }, (_, i) => (
        <div key={i} className="flex flex-col gap-2 border-b border-rule py-3 pr-4 pl-6">
          <div className="flex justify-between gap-4">
            <Skeleton className="h-3.5 w-1/3" />
            <Skeleton className="h-3 w-12" />
          </div>
          <Skeleton className="h-3 w-11/12" />
          <Skeleton className="h-4 w-20" />
        </div>
      ))}
    </div>
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
  const [selecting, setSelecting] = useState(false);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const del = useDeleteMessages(ws);
  const notify = useToast();
  const toggle = (id: string) =>
    setSelected((prev) => {
      const next = new Set(prev);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  const stopSelecting = () => {
    setSelecting(false);
    setSelected(new Set());
  };
  const closeConfirm = () => {
    del.reset();
    setConfirming(false);
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-col gap-3 border-b border-rule-strong px-4 pt-5 pb-4">
        <div>
          <div className="flex items-center justify-between gap-2">
            <p className="kicker">Incoming</p>
            <Button variant="ghost" size="sm" aria-pressed={selecting} onClick={() => (selecting ? stopSelecting() : setSelecting(true))}>
              {selecting ? 'Done' : 'Select'}
            </Button>
          </div>
          <h1 className="type-display mt-1 text-[2rem] leading-none tracking-[-0.02em]">Inbox</h1>
          <p className="mt-2 text-sm text-ink-muted">Every email that reaches this workspace, read and labelled for you.</p>
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
        {selecting && (
          <div role="group" aria-label="Selected emails" className="flex flex-wrap items-center gap-2 border border-rule-strong bg-raised px-3 py-2 text-sm">
            <span className="mr-auto" aria-live="polite">
              {selected.size} selected
            </span>
            <Button variant="ghost" size="sm" onClick={() => setSelected(new Set(items.map((m) => m.id)))} disabled={items.length === 0}>
              Select all loaded
            </Button>
            <Button variant="danger" size="sm" disabled={selected.size === 0} onClick={() => setConfirming(true)}>
              Delete
            </Button>
          </div>
        )}
        <SegmentedControl
          label="Sort messages"
          options={SORT_OPTIONS}
          value={sort}
          onChange={(next) => {
            setSort(next);
            writeSort(next);
          }}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
        {messages.isLoading && <ListSkeleton />}
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
                : 'Emails that reach this workspace will appear here, each one labelled. Forward one to your inbound address or send a test email from Integrations.'
            }
          />
        )}
        <ul aria-label="Messages in this view">
          {items.map((m) => (
            <li key={m.id} className="border-b border-rule">
              {selecting ? <SelectableRow ws={ws} m={m} checked={selected.has(m.id)} onToggle={() => toggle(m.id)} /> : <Row ws={ws} m={m} />}
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
      {confirming && (
        <ConfirmDialog
          title="Delete from Post Room?"
          confirmLabel={selected.size === 1 ? 'Delete 1 email' : `Delete ${selected.size} emails`}
          busy={del.isPending}
          error={del.error}
          onCancel={closeConfirm}
          onConfirm={() =>
            del.mutate(
              { ids: [...selected] },
              {
                onSuccess: (res) => {
                  notify('success', deletedToast(res.deleted));
                  closeConfirm();
                  stopSelecting();
                },
              },
            )
          }
        >
          {DELETE_NOTE} The {selected.size === 1 ? 'email, its' : `${selected.size} emails, their`} attachments, summaries and drafts are removed from Post Room, and they will not come back when Gmail syncs. This can&apos;t be undone.
        </ConfirmDialog>
      )}
    </div>
  );
}
