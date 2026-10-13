import { useState, type CSSProperties, type ReactNode } from 'react';
import { ApiError } from '../../api/client';
import type { Source } from '../../api/types';
import { Badge, Button, Card, CardTitle, Mono, PageHeader, Spinner } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { useWorkspace } from '../../app/useWorkspace';
import { cn } from '../../lib/cn';
import { fullDate } from '../../lib/format';
import Nango from '@nangohq/frontend';
import {
  useCreateForwardingSource,
  useDisconnectSource,
  useNangoSession,
  useRegisterNangoConnection,
  useSendTestEmail,
  useSources,
  useSyncSource,
} from './hooks';

function capabilityLabel(source: Source): string {
  return source.capabilities.provider_drafts ? 'Draft capable' : 'Read only';
}

function Ticket({
  number,
  title,
  chips,
  className,
  style,
  children,
}: {
  number: string;
  title: string;
  chips?: ReactNode;
  className?: string;
  style?: CSSProperties;
  children: ReactNode;
}) {
  return (
    <Card className={cn('rise relative flex flex-col gap-4 overflow-hidden', className)} style={style}>
      <div className="flex items-start justify-between gap-3">
        <div>
          <p className="kicker">Service {number}</p>
          <CardTitle className="mt-1 text-2xl">{title}</CardTitle>
        </div>
        {chips && <div className="flex flex-wrap justify-end gap-1.5">{chips}</div>}
      </div>
      <div aria-hidden="true" className="relative border-t border-dashed border-rule-strong">
        <span className="absolute -top-2 -left-[25px] h-4 w-4 rounded-full border border-rule bg-paper" />
        <span className="absolute -top-2 -right-[25px] h-4 w-4 rounded-full border border-rule bg-paper" />
      </div>
      {children}
    </Card>
  );
}

function ForwardingCard({ ws, source, isAdmin, style }: { ws: string; source: Source | undefined; isAdmin: boolean; style?: CSSProperties }) {
  const create = useCreateForwardingSource(ws);
  return (
    <Ticket
      number="01"
      title="Forward emails"
      style={style}
      chips={
        source && (
          <>
            <StatusBadge status={source.status} />
            <Badge tone="neutral">{capabilityLabel(source)}</Badge>
          </>
        )
      }
    >
      <p className="text-sm text-ink-muted">
        Forward HR emails to a dedicated address. Messages are read, classified and routed; nothing is ever sent from your mailbox.
      </p>
      {source ? (
        <ForwardingDetails ws={ws} source={source} isAdmin={isAdmin} />
      ) : isAdmin ? (
        <div className="flex flex-col gap-2">
          <Button onClick={() => create.mutate()} loading={create.isPending} className="self-start">
            Create forwarding address
          </Button>
          <ErrorNote error={create.error} />
        </div>
      ) : (
        <p className="text-sm text-ink-muted">Ask a workspace admin to set up forwarding.</p>
      )}
    </Ticket>
  );
}

function ForwardingDetails({ ws, source, isAdmin }: { ws: string; source: Source; isAdmin: boolean }) {
  const [copied, setCopied] = useState(false);
  const [testUnavailable, setTestUnavailable] = useState(false);
  const test = useSendTestEmail(ws, source.id);
  const address = source.inbound_address ?? '';

  const copy = async () => {
    await navigator.clipboard.writeText(address);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
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
    <div className="flex flex-col gap-3">
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
        <div className="flex items-center gap-3">
          <Button variant="secondary" size="sm" onClick={sendTest} loading={test.isPending}>
            Send test email
          </Button>
          {test.isSuccess && <span className="text-sm text-success">Test email queued. Check the inbox.</span>}
        </div>
      )}
      {test.error && !testUnavailable && <ErrorNote error={test.error} />}
    </div>
  );
}

function GmailCard({ ws, source, isAdmin, style }: { ws: string; source: Source | undefined; isAdmin: boolean; style?: CSSProperties }) {
  const connected = source && source.status !== 'disconnected';
  return (
    <Ticket
      number="02"
      title="Connect Gmail"
      style={style}
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
    >
      <p className="text-sm text-ink-muted">
        Connect a shared HR Gmail inbox through Nango. We import the last 14 days (up to 500 messages), then new mail as it
        arrives. Drafts stay in this app; nothing is sent from the mailbox.
      </p>
      {connected ? (
        <GmailDetails ws={ws} source={source} isAdmin={isAdmin} />
      ) : isAdmin ? (
        <ConnectGmailButton ws={ws} />
      ) : (
        <p className="text-sm text-ink-muted">Ask a workspace admin to connect Gmail.</p>
      )}
    </Ticket>
  );
}

function ConnectGmailButton({ ws }: { ws: string }) {
  const session = useNangoSession(ws);
  const register = useRegisterNangoConnection(ws);
  const [uiError, setUiError] = useState<string | null>(null);
  const notConfigured = session.error instanceof ApiError && session.error.status === 501;

  const connect = () => {
    setUiError(null);
    // Open the Connect UI synchronously (keeps the click gesture), then hand it the token.
    const ui = new Nango().openConnectUI({
      onEvent: (event) => {
        if (event.type === 'connect') {
          register.mutate({ connection_id: event.payload.connectionId, provider_config_key: event.payload.providerConfigKey });
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
    <div className="flex flex-col gap-2">
      <Button onClick={connect} loading={session.isPending || register.isPending} className="self-start">
        Connect Gmail
      </Button>
      {register.isPending && <span className="text-sm text-ink-muted">Verifying connection…</span>}
      {notConfigured ? (
        <p className="text-sm text-ink-muted">Nango isn&apos;t configured on the server yet. Set NANGO_API_KEY to enable this.</p>
      ) : (
        <ErrorNote error={session.error} />
      )}
      <ErrorNote error={register.error} />
      {uiError && <p className="text-sm text-danger">{uiError}</p>}
    </div>
  );
}

function GmailDetails({ ws, source, isAdmin }: { ws: string; source: Source; isAdmin: boolean }) {
  const sync = useSyncSource(ws);
  const disconnect = useDisconnectSource(ws);
  const [confirming, setConfirming] = useState(false);
  return (
    <div className="flex flex-col gap-3">
      <Mono>
        Connected {fullDate(source.created_at)}
        {source.last_synced_at ? ` · Last sync ${fullDate(source.last_synced_at)}` : ' · First sync running…'}
      </Mono>
      {source.last_error && (
        <p role="status" className="rounded-sm border border-danger/40 bg-danger-tint px-3 py-2 text-sm text-danger">
          {source.last_error}
        </p>
      )}
      {isAdmin && (
        <div className="flex flex-wrap items-center gap-3">
          <Button variant="secondary" size="sm" onClick={() => sync.mutate(source.id)} loading={sync.isPending}>
            Sync now
          </Button>
          {confirming ? (
            <>
              <Button variant="danger" size="sm" onClick={() => disconnect.mutate(source.id)} loading={disconnect.isPending}>
                Confirm disconnect
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setConfirming(false)}>
                Cancel
              </Button>
            </>
          ) : (
            <Button variant="ghost" size="sm" onClick={() => setConfirming(true)}>
              Disconnect
            </Button>
          )}
          {sync.isSuccess && <span className="text-sm text-success">Sync queued.</span>}
        </div>
      )}
      <ErrorNote error={sync.error ?? disconnect.error} />
    </div>
  );
}

function MicrosoftCard({ style }: { style?: CSSProperties }) {
  return (
    <Ticket number="03" title="Connect Microsoft 365" style={style} chips={<Badge>Not yet supported</Badge>} className="bg-neutral-tint/40">
      <p className="text-sm text-ink-muted">
        Outlook mailboxes will use the same Nango flow once the Microsoft proof of concept passes (message IDs, threading,
        delta sync and tenant-admin consent).
      </p>
      <Button variant="secondary" size="sm" disabled className="self-start">
        Not yet supported
      </Button>
    </Ticket>
  );
}

function MoreSourcesCard({ style }: { style?: CSSProperties }) {
  return (
    <Ticket number="04" title="More coming later" style={style} chips={<Badge>Planned</Badge>} className="bg-neutral-tint/40">
      <p className="text-sm text-ink-muted">Additional providers and sources are on the roadmap.</p>
    </Ticket>
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
    <div className="mx-auto flex max-w-5xl flex-col gap-8 px-10 py-10">
      <PageHeader kicker="Postal services" title="Integrations" description="Choose how emails reach this workspace." />
      {sources.isLoading && <Spinner />}
      <ErrorNote error={sources.error} />
      {sources.data && (
        <div className="grid items-start gap-6 md:grid-cols-2">
          <ForwardingCard ws={ws} source={forwarding} isAdmin={isAdmin} style={riseStyle(1)} />
          <GmailCard ws={ws} source={gmail} isAdmin={isAdmin} style={riseStyle(2)} />
          <MicrosoftCard style={riseStyle(3)} />
          <MoreSourcesCard style={riseStyle(4)} />
        </div>
      )}
    </div>
  );
}
