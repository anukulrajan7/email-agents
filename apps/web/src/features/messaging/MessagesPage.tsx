import { useState } from 'react';
import { Link } from 'react-router';
import { ErrorNote } from '../../components/ErrorNote';
import { Badge, Button, Card, EmptyState, Mono, Page, PageHeader, RelativeTime, SkeletonList, Tabs, Textarea, useToast, type TabItem, type Tone } from '../../components/ui';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { useApproveSend, useConversations, useInApp, useMarkRead, useReply, useSentByMeera, useStop, useStopWatching, useTicketStatus } from './hooks';
import type { Conversation, InboxItem, SentItem } from './types';

type Tab = 'inbox' | 'sent' | 'handling';

const SENT_LABEL: Record<string, { label: string; tone: Tone }> = {
  needs_you: { label: 'Needs your OK', tone: 'warning' },
  scheduled: { label: 'Waiting to go out', tone: 'info' },
  held: { label: 'Waiting to go out', tone: 'info' },
  sent: { label: 'Sent', tone: 'success' },
  delivered: { label: 'Delivered', tone: 'success' },
  read: { label: 'Read', tone: 'success' },
  stopped: { label: 'Stopped', tone: 'neutral' },
  cancelled: { label: 'Stopped', tone: 'neutral' },
  blocked: { label: 'Not sent', tone: 'danger' },
};

const CONVERSATION_TONE: Record<Conversation['status'], Tone> = {
  waiting: 'info',
  starting: 'info',
  nudged: 'warning',
  open: 'warning',
  answered: 'success',
  resolved: 'success',
};

function linkFor(ws: string, link: InboxItem['link']): { to: string; label: string } | undefined {
  if (link?.kind === 'message' && link.id) return { to: `/w/${ws}/inbox/${link.id}`, label: 'Open the email' };
  if (link?.kind === 'case' && link.id) return { to: `/w/${ws}/agents/${link.id}`, label: 'Open the case' };
  return undefined;
}

function InboxRow({ ws, item }: { ws: string; item: InboxItem }) {
  const [replying, setReplying] = useState(false);
  const [text, setText] = useState('');
  const notify = useToast();
  const read = useMarkRead(ws);
  const reply = useReply(ws);
  const link = linkFor(ws, item.link);
  return (
    <li>
      <Card className="flex flex-col gap-2" role="group" aria-label={item.subject}>
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone={item.kind === 'notice' ? 'info' : 'neutral'}>{item.kind === 'notice' ? 'Notice' : 'Message'}</Badge>
          {!item.read && <Badge tone="warning">New</Badge>}
          <Mono className="ml-auto">
            <RelativeTime iso={item.at} />
          </Mono>
        </div>
        <h3 className="text-[15px] font-semibold text-ink">{item.subject}</h3>
        <p className="text-[13px] text-ink-muted">From {item.from_assistant ? `${item.from}'s assistant` : item.from}</p>
        <p className="text-sm break-words whitespace-pre-wrap text-ink">{item.body}</p>
        <div className="flex flex-wrap items-center gap-2">
          {link && (
            <Link to={link.to} className="text-sm text-accent underline decoration-accent/40 underline-offset-[3px]">
              {link.label}
            </Link>
          )}
          {!item.read && (
            <Button size="sm" variant="ghost" loading={read.isPending} onClick={() => read.mutate(item.id)}>
              Mark as read
            </Button>
          )}
          {item.can_reply && !replying && (
            <Button size="sm" variant="secondary" onClick={() => setReplying(true)}>
              Reply
            </Button>
          )}
        </div>
        {replying && (
          <form
            className="flex flex-col gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              reply.mutate(
                { id: item.id, body: text },
                {
                  onSuccess: () => {
                    notify('success', 'Reply sent.');
                    setReplying(false);
                    setText('');
                  },
                  onError: (err) => notify('error', err instanceof Error ? err.message : 'Could not send the reply.'),
                },
              );
            }}
          >
            <Textarea rows={3} value={text} onChange={(e) => setText(e.target.value)} aria-label={`Your reply to ${item.subject}`} maxLength={4000} />
            <div className="flex gap-2">
              <Button size="sm" type="submit" disabled={!text.trim()} loading={reply.isPending}>
                Send reply
              </Button>
              <Button size="sm" variant="ghost" type="button" onClick={() => setReplying(false)}>
                Cancel
              </Button>
            </div>
          </form>
        )}
      </Card>
    </li>
  );
}

function InboxTab({ ws }: { ws: string }) {
  const inbox = useInApp(ws);
  return (
    <div className="flex flex-col gap-4">
      {inbox.isLoading && <SkeletonList rows={3} label="Loading messages" />}
      <ErrorNote error={inbox.error} />
      {inbox.data &&
        (inbox.data.items.length === 0 ? (
          <Card>
            <EmptyState title="Nothing new" description="Messages from people here, and notices from your assistant such as “Support replied”, show up on this page." />
          </Card>
        ) : (
          <ul className="flex flex-col gap-3">
            {inbox.data.items.map((m) => (
              <InboxRow key={m.id} ws={ws} item={m} />
            ))}
          </ul>
        ))}
    </div>
  );
}

function SentRow({ ws, item }: { ws: string; item: SentItem }) {
  const notify = useToast();
  const stop = useStop(ws);
  const approve = useApproveSend(ws);
  const state = SENT_LABEL[item.status] ?? { label: item.status, tone: 'neutral' as const };
  const fail = (e: unknown) => notify('error', e instanceof Error ? e.message : 'Something went wrong.');
  return (
    <li>
      <Card className="flex flex-col gap-2" role="group" aria-label={item.subject}>
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone={state.tone}>{state.label}</Badge>
          <Badge tone="neutral">{item.channel === 'email' ? 'Email' : 'In the app'}</Badge>
          <Mono className="ml-auto">
            <RelativeTime iso={item.at} />
          </Mono>
        </div>
        <h3 className="text-[15px] font-semibold text-ink">{item.subject}</h3>
        <p className="text-[13px] text-ink-muted">To {item.to}</p>
        <p className="text-sm break-words whitespace-pre-wrap text-ink">{item.body}</p>
        {item.reason && item.status === 'needs_you' && <p className="text-[13px] text-ink-muted">Waiting for you because {item.reason}.</p>}
        {item.error && <p className="text-[13px] text-danger">{item.error}</p>}
        <div className="flex gap-2">
          {item.can_approve && (
            <Button size="sm" loading={approve.isPending} onClick={() => approve.mutate(item.id, { onSuccess: () => notify('success', 'It will go out in a moment.'), onError: fail })}>
              Send it
            </Button>
          )}
          {item.can_stop && (
            <Button size="sm" variant="secondary" loading={stop.isPending} onClick={() => stop.mutate(item.id, { onSuccess: () => notify('success', 'Stopped. Nothing was sent.'), onError: fail })}>
              Stop
            </Button>
          )}
        </div>
      </Card>
    </li>
  );
}

function SentTab({ ws }: { ws: string }) {
  const sent = useSentByMeera(ws);
  return (
    <div className="flex flex-col gap-4">
      {sent.isLoading && <SkeletonList rows={3} label="Loading sent messages" />}
      <ErrorNote error={sent.error} />
      {sent.data &&
        (sent.data.items.length === 0 ? (
          <Card>
            <EmptyState title="Nothing sent yet" description="Ask Meera to write to someone. What she sends for you is listed here, and you can stop it until it goes out." />
          </Card>
        ) : (
          <ul className="flex flex-col gap-3">
            {sent.data.items.map((m) => (
              <SentRow key={m.id} ws={ws} item={m} />
            ))}
          </ul>
        ))}
    </div>
  );
}

function ConversationRow({ ws, item }: { ws: string; item: Conversation }) {
  const notify = useToast();
  const close = useStopWatching(ws);
  const ticket = useTicketStatus(ws);
  const fail = (e: unknown) => notify('error', e instanceof Error ? e.message : 'Something went wrong.');
  return (
    <li>
      <Card className="flex flex-col gap-2" role="group" aria-label={item.title}>
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone={CONVERSATION_TONE[item.status]}>{item.status_label}</Badge>
          <Badge tone="neutral">{item.kind === 'ticket' ? 'Support case' : item.kind === 'desk' ? 'With the team' : 'Question'}</Badge>
          <Mono className="ml-auto">
            <RelativeTime iso={item.updated_at} />
          </Mono>
        </div>
        <h3 className="text-[15px] font-semibold text-ink">{item.title}</h3>
        <p className="text-[13px] text-ink-muted">With {item.with}</p>
        {item.answer && <p className="text-sm break-words whitespace-pre-wrap text-ink">{item.answer}</p>}
        {item.ticket && item.ticket.last_summary && !item.answer && <p className="text-sm text-ink">{item.ticket.last_summary}</p>}
        <div className="flex gap-2">
          {item.ticket && item.ticket.status !== 'resolved' && (
            <Button
              size="sm"
              variant="secondary"
              loading={ticket.isPending}
              onClick={() => ticket.mutate({ id: item.ticket!.id, status: 'resolved' }, { onSuccess: () => notify('success', 'Marked as resolved.'), onError: fail })}
            >
              Mark resolved
            </Button>
          )}
          <Button size="sm" variant="ghost" loading={close.isPending} onClick={() => close.mutate(item.id, { onSuccess: () => notify('success', 'I will stop watching this.'), onError: fail })}>
            Stop watching
          </Button>
        </div>
      </Card>
    </li>
  );
}

function HandlingTab({ ws }: { ws: string }) {
  const list = useConversations(ws);
  return (
    <div className="flex flex-col gap-4">
      {list.isLoading && <SkeletonList rows={3} label="Loading conversations" />}
      <ErrorNote error={list.error} />
      {list.data &&
        (list.data.items.length === 0 ? (
          <Card>
            <EmptyState title="Nothing in progress" description="When Meera asks someone a question or writes to a company for you, she keeps track here and tells you the answer." />
          </Card>
        ) : (
          <ul className="flex flex-col gap-3">
            {list.data.items.map((c) => (
              <ConversationRow key={c.id} ws={ws} item={c} />
            ))}
          </ul>
        ))}
    </div>
  );
}

export function MessagesPage() {
  const { ws } = useWorkspace();
  const [tab, setTab] = useState<Tab>('inbox');
  const inbox = useInApp(ws);
  const handling = useConversations(ws);
  const tabs: TabItem<Tab>[] = [
    { id: 'inbox', label: 'Messages', ...(inbox.data ? { count: inbox.data.unread } : {}) },
    { id: 'sent', label: 'Sent by Meera' },
    { id: 'handling', label: 'Conversations Meera is handling', ...(handling.data ? { count: handling.data.items.length } : {}) },
  ];
  return (
    <Page className="gap-6">
      <PageHeader
        kicker="Between people and assistants"
        title="Messages"
        description="Messages from people here, notices from your assistant, what she sent for you, and the conversations she is following up."
      />
      <div className="rise" style={riseStyle(1)}>
        <Tabs label="Message views" tabs={tabs} value={tab} onChange={setTab}>
          {tab === 'inbox' ? <InboxTab ws={ws} /> : tab === 'sent' ? <SentTab ws={ws} /> : <HandlingTab ws={ws} />}
        </Tabs>
      </div>
    </Page>
  );
}
