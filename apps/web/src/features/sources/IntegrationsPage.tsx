import { useState, type CSSProperties, type ReactNode } from 'react';
import { ApiError } from '../../api/client';
import type { Source } from '../../api/types';
import { Badge, Button, Card, CardTitle, ConfirmDialog, Mono, Page, PageHeader, Skeleton, useToast } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { useWorkspace } from '../../app/useWorkspace';
import { cn } from '../../lib/cn';
import { fullDate } from '../../lib/format';
import type { PurgePreview } from '../../api/types';
import Nango from '@nangohq/frontend';
import {
  useCreateForwardingSource,
  useDisconnectSource,
  useNangoSession,
  useRegisterNangoConnection,
  useSendTestEmail,
  useSendTestEmailEnsuringSource,
  useSources,
  usePurgePreview,
  useSyncSource,
} from './hooks';

const count = new Intl.NumberFormat('en');
const plural = (n: number, one: string, many = `${one}s`) => `${count.format(n)} ${n === 1 ? one : many}`;

/** The exact counts the confirm dialog promises will be deleted. */
export function purgeSentence(c: PurgePreview): string {
  return `${plural(c.emails, 'email')}, ${plural(c.attachments, 'attachment')}, ${plural(c.drafts, 'draft')} and their summaries will be permanently deleted from Post Room. Labels, settings and other mailboxes stay. This can't be undone.`;
}

const TEST_QUEUED = 'Test email queued — it appears in the Inbox in a few seconds';

function capabilityLabel(source: Source): string {
  return source.capabilities.provider_drafts ? 'Can save drafts' : 'Read only';
}

function Ticket({
  number,
  title,
  chips,
  description,
  footer,
  muted = false,
  style,
  children,
}: {
  number: string;
  title: string;
  chips?: ReactNode;
  description: string;
  footer?: ReactNode;
  muted?: boolean;
  style?: CSSProperties;
  children?: ReactNode;
}) {
  return (
    <Card className={cn('rise relative flex h-full flex-col gap-4 overflow-hidden p-6', muted && 'bg-neutral-tint/40')} style={style}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="kicker">Service {number}</p>
          <CardTitle className="mt-1 text-2xl">{title}</CardTitle>
        </div>
        {chips && <div className="flex flex-wrap justify-end gap-1.5">{chips}</div>}
      </div>
      <div aria-hidden="true" className="relative border-t border-dashed border-rule-strong">
        <span className="absolute -top-2 -left-[33px] h-4 w-4 rounded-full border border-rule bg-paper" />
        <span className="absolute -top-2 -right-[33px] h-4 w-4 rounded-full border border-rule bg-paper" />
      </div>
      <p className="text-sm leading-relaxed text-ink-muted">{description}</p>
      {children}
      {footer && <div className="mt-auto flex flex-col gap-2 border-t border-rule pt-4">{footer}</div>}
    </Card>
  );
}

function ForwardingCard({ ws, source, isAdmin, style }: { ws: string; source: Source | undefined; isAdmin: boolean; style?: CSSProperties }) {
  const { hasStaffing } = useWorkspace();
  const create = useCreateForwardingSource(ws);
  const test = useSendTestEmailEnsuringSource(ws);
  const notify = useToast();
  return (
    <Ticket
      number="01"
      title="Forward emails"
      style={style}
      description={`Forward ${hasStaffing ? 'HR ' : ''}emails to a private address. Each one is read, labelled and routed; nothing is ever sent from your mailbox.`}
      chips={
        source && (
          <>
            <StatusBadge status={source.status} />
            <Badge tone="neutral">{capabilityLabel(source)}</Badge>
          </>
        )
      }
      footer={
        source ? undefined : isAdmin ? (
          <>
            <div className="flex flex-wrap items-center gap-2">
              <Button onClick={() => create.mutate(undefined, { onSuccess: () => notify('success', 'Forwarding address created.') })} loading={create.isPending}>
                Create forwarding address
              </Button>
              <Button variant="secondary" onClick={() => test.mutate()} loading={test.isPending}>
                Send test email
              </Button>
            </div>
            {test.isSuccess && (
              <span role="status" className="text-sm text-success">
                {TEST_QUEUED}
              </span>
            )}
            <ErrorNote error={create.error ?? test.error} />
          </>
        ) : (
          <p className="text-sm text-ink-muted">Ask a workspace admin to set up forwarding.</p>
        )
      }
    >
      {source && <ForwardingDetails ws={ws} source={source} isAdmin={isAdmin} />}
    </Ticket>
  );
}

function ForwardingDetails({ ws, source, isAdmin }: { ws: string; source: Source; isAdmin: boolean }) {
  const [copied, setCopied] = useState(false);
  const [testUnavailable, setTestUnavailable] = useState(false);
  const test = useSendTestEmail(ws, source.id);
  const notify = useToast();
  const address = source.inbound_address ?? '';

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(address);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      notify('error', 'Could not copy. Select the address and copy it yourself.');
    }
  };

  const sendTest = () =>
    test.mutate(
      {},
      {
        onError: (err) => {
          if (err instanceof ApiError && err.status === 404) setTestUnavailable(true);
        },
      },
    );

  return (
    <div className="flex flex-1 flex-col gap-3">
      <div className="overflow-hidden rounded-sm border border-rule-strong bg-raised">
        <div aria-hidden="true" className="airmail h-1" />
        <div className="flex items-center gap-3 p-3">
          <div className="min-w-0 flex-1">
            <p className="kicker">Inbound address</p>
            <code className="mt-1 block font-mono text-[13px] break-all text-ink" title={address}>
              {address || 'Pending'}
            </code>
          </div>
          <Button variant="secondary" size="sm" onClick={copy} disabled={!address} aria-label="Copy inbound address" className="w-[4.5rem] shrink-0">
            {copied ? 'Copied' : 'Copy'}
          </Button>
        </div>
      </div>
      <Mono>
        Created {fullDate(source.created_at)}
        {source.last_synced_at ? ` · Last email ${fullDate(source.last_synced_at)}` : ''}
      </Mono>
      {isAdmin && !testUnavailable && (
        <div className="mt-auto flex flex-wrap items-center gap-3 border-t border-rule pt-4">
          <Button variant="secondary" size="sm" onClick={sendTest} loading={test.isPending}>
            Send test email
          </Button>
          {test.isSuccess && (
            <span role="status" className="text-sm text-success">
              {TEST_QUEUED}
            </span>
          )}
        </div>
      )}
      {test.error && !testUnavailable && <ErrorNote error={test.error} />}
    </div>
  );
}

function GmailCard({ ws, source, isAdmin, style }: { ws: string; source: Source | undefined; isAdmin: boolean; style?: CSSProperties }) {
  const { hasStaffing } = useWorkspace();
  const connected = source && source.status !== 'disconnected';
  const purging = source?.purge?.status === 'running' ? source.purge : undefined;
  return (
    <Ticket
      number="02"
      title="Connect Gmail"
      style={style}
      description={`Connect a shared ${hasStaffing ? 'HR ' : ''}Gmail inbox. We bring in the last 14 days (up to 500 messages), then new mail as it arrives. Nothing is sent from the mailbox without your OK.`}
      chips={
        connected ? (
          <>
            <StatusBadge status={source.status} />
            <Badge tone="neutral">{capabilityLabel(source)}</Badge>
          </>
        ) : (
          <Badge tone="neutral">Read + sync</Badge>
        )
      }
      footer={
        connected || purging ? undefined : isAdmin ? (
          <ConnectGmailButton ws={ws} />
        ) : (
          <p className="text-sm text-ink-muted">Ask a workspace admin to connect Gmail.</p>
        )
      }
    >
      {purging && (
        <div role="status" className="flex flex-col gap-2 text-sm">
          <p className="font-semibold">
            Deleting… {count.format(purging.done)}/{count.format(purging.total)}
          </p>
          <progress aria-label="Deleting mail from Post Room" max={Math.max(purging.total, 1)} value={purging.done} className="h-2 w-full accent-accent" />
          <p className="text-ink-muted">Everything from this mailbox is being removed from Post Room. You can leave this page.</p>
        </div>
      )}
      {connected && <GmailDetails ws={ws} source={source} isAdmin={isAdmin} />}
    </Ticket>
  );
}

function ConnectGmailButton({ ws }: { ws: string }) {
  const session = useNangoSession(ws);
  const register = useRegisterNangoConnection(ws);
  const notify = useToast();
  const [uiError, setUiError] = useState<string | undefined>();
  const notConfigured = session.error instanceof ApiError && session.error.status === 501;

  const connect = () => {
    setUiError(undefined);
    // Open the Connect UI synchronously (keeps the click gesture), then hand it the token.
    const ui = new Nango().openConnectUI({
      onEvent: (event) => {
        if (event.type === 'connect') {
          register.mutate(
            { connection_id: event.payload.connectionId, provider_config_key: event.payload.providerConfigKey },
            { onSuccess: () => notify('success', 'Gmail connected. Your recent mail is on its way.') },
          );
        } else if (event.type === 'error') {
          setUiError(event.payload.errorMessage);
        }
      },
    });
    session.mutate('gmail', {
      onSuccess: (s) => ui.setSessionToken(s.token),
      onError: () => ui.close(),
    });
  };

  return (
    <>
      <Button onClick={connect} loading={session.isPending || register.isPending} className="self-start">
        Connect Gmail
      </Button>
      {register.isPending && (
        <span role="status" className="text-sm text-ink-muted">
          Checking the connection…
        </span>
      )}
      {notConfigured ? (
        <p className="text-sm text-ink-muted">Connecting Gmail isn&apos;t switched on for this server yet. Ask whoever runs Post Room to finish the mailbox setup.</p>
      ) : (
        <ErrorNote error={session.error} />
      )}
      <ErrorNote error={register.error} />
      {uiError && (
        <p role="alert" className="text-sm text-danger">
          {uiError}
        </p>
      )}
    </>
  );
}

function GmailDetails({ ws, source, isAdmin }: { ws: string; source: Source; isAdmin: boolean }) {
  const sync = useSyncSource(ws);
  const disconnect = useDisconnectSource(ws);
  const notify = useToast();
  const [confirming, setConfirming] = useState(false);
  const preview = usePurgePreview(ws, source.id, confirming);
  return (
    <div className="flex flex-1 flex-col gap-3">
      <Mono>
        Connected {fullDate(source.created_at)}
        {source.last_synced_at ? ` · Last sync ${fullDate(source.last_synced_at)}` : ' · First sync running…'}
      </Mono>
      {source.last_error && (
        <p role="status" className="rounded-sm border-l-[3px] border-danger bg-danger-tint px-3 py-2 text-sm text-danger">
          {source.last_error}
        </p>
      )}
      {isAdmin && (
        <div className="mt-auto flex flex-wrap items-center gap-2 border-t border-rule pt-4">
          <Button
            variant="secondary"
            size="sm"
            onClick={() => sync.mutate(source.id, { onSuccess: () => notify('success', 'Sync started. New mail will show up shortly.') })}
            loading={sync.isPending}
          >
            Sync now
          </Button>
          <Button variant="ghost" size="sm" onClick={() => setConfirming(true)}>
            Disconnect
          </Button>
        </div>
      )}
      <ErrorNote error={sync.error} />
      {confirming && (
        <ConfirmDialog
          title="Disconnect Gmail?"
          confirmLabel="Disconnect and delete"
          busy={disconnect.isPending}
          confirmDisabled={!preview.data}
          error={disconnect.error ?? preview.error}
          onCancel={() => {
            disconnect.reset();
            setConfirming(false);
          }}
          onConfirm={() =>
            disconnect.mutate(source.id, {
              onSuccess: () => {
                setConfirming(false);
                notify('success', 'Gmail disconnected. Its mail is being deleted from Post Room.');
              },
            })
          }
        >
          <p>New mail from this inbox stops arriving.</p>
          <p className="mt-2 font-semibold text-ink">{preview.data ? purgeSentence(preview.data) : 'Counting what will be deleted…'}</p>
        </ConfirmDialog>
      )}
    </div>
  );
}

function MicrosoftCard({ style }: { style?: CSSProperties }) {
  return (
    <Ticket
      number="03"
      title="Connect Microsoft 365"
      style={style}
      muted
      chips={<Badge>Coming soon</Badge>}
      description="Outlook and Microsoft 365 mailboxes will connect the same way as Gmail. We will let you know when it is ready."
      footer={
        <Button variant="secondary" size="sm" disabled className="self-start">
          Not available yet
        </Button>
      }
    />
  );
}

function MoreSourcesCard({ style }: { style?: CSSProperties }) {
  return (
    <Ticket
      number="04"
      title="More coming later"
      style={style}
      muted
      chips={<Badge>Planned</Badge>}
      description="More mail providers and sources are on the way. Tell us which one your team needs next."
    />
  );
}

function SourcesSkeleton() {
  return (
    <div role="status" aria-label="Loading integrations" className="grid gap-6 md:grid-cols-2 2xl:grid-cols-3">
      {[0, 1].map((i) => (
        <div key={i} className="flex h-64 flex-col gap-4 rounded-sm border border-rule bg-surface p-6">
          <Skeleton className="h-3 w-20" />
          <Skeleton className="h-7 w-48" />
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-4/5" />
          <Skeleton className="mt-auto h-10 w-40" />
        </div>
      ))}
    </div>
  );
}

export function IntegrationsPage() {
  const { ws, isAdmin } = useWorkspace();
  const sources = useSources(ws, { poll: true });
  const forwarding = sources.data?.items.find((s) => s.kind === 'forwarding');
  // Prefer a live connection; fall back to the most recent disconnected one.
  const gmailSources = sources.data?.items.filter((s) => s.kind === 'nango_gmail') ?? [];
  const gmail = gmailSources.find((s) => s.status !== 'disconnected') ?? gmailSources.at(-1);

  return (
    <Page>
      <PageHeader kicker="Postal services" title="Integrations" description="Where your email comes from. Connect a mailbox or forward mail to a private address." />
      {sources.isLoading && <SourcesSkeleton />}
      <ErrorNote error={sources.error} />
      {sources.data && (
        <div className="grid auto-rows-fr gap-6 md:grid-cols-2 2xl:grid-cols-3">
          <ForwardingCard ws={ws} source={forwarding} isAdmin={isAdmin} style={riseStyle(1)} />
          <GmailCard ws={ws} source={gmail} isAdmin={isAdmin} style={riseStyle(2)} />
          <MicrosoftCard style={riseStyle(3)} />
          <MoreSourcesCard style={riseStyle(4)} />
        </div>
      )}
    </Page>
  );
}
