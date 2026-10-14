import { useState } from 'react';
import { Link } from 'react-router';
import { Button, Page, PageHeader, Skeleton } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { cn } from '../../lib/cn';
import { riseStyle } from '../../lib/motion';
import { AgentFace } from '../agents-hub/AgentFace';
import { AvatarPicker } from '../agents-hub/AvatarPicker';
import { resolveAvatar } from '../agents-hub/avatars';
import { useAgentRegistry, usePatchAgent } from '../agents-hub/hooks';
import { HelpPopover } from '../help/HelpPopover';
import { useMessages } from '../inbox/hooks';
import { useSendTestEmailEnsuringSource, useSources } from '../sources/hooks';

type Step = 'connect' | 'meet' | 'win';
const STEPS: { id: Step; label: string }[] = [
  { id: 'connect', label: 'Connect your mail' },
  { id: 'meet', label: 'Meet your agents' },
  { id: 'win', label: 'Your first win' },
];

function ConnectStep({ ws, onNext }: { ws: string; onNext: () => void }) {
  const sources = useSources(ws);
  const testEmail = useSendTestEmailEnsuringSource(ws);
  const connected = sources.data?.items.some((s) => s.status === 'connected') ?? false;
  return (
    <div className="flex flex-col gap-4">
      <p className="max-w-[60ch] text-[15px] leading-relaxed text-ink-muted">
        Your agents can only help with mail they can see. Connect Gmail or Outlook, or use a forwarding address. They start by watching and drafting, and never send without asking.
      </p>
      {connected && (
        <p role="status" className="rounded-sm border-l-[3px] border-success bg-success-tint px-3 py-2 text-sm text-success">
          A mailbox is connected.
        </p>
      )}
      <ErrorNote error={sources.error ?? testEmail.error} />
      <div className="flex flex-wrap gap-2">
        <Link
          to={`/w/${ws}/integrations`}
          className="inline-flex h-10 items-center rounded-sm bg-accent px-4 text-sm font-semibold text-accent-ink hover:bg-accent-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
        >
          Connect a mailbox
        </Link>
        <Button variant="secondary" loading={testEmail.isPending} onClick={() => testEmail.mutate()}>
          Send a test email instead
        </Button>
        <Button variant={connected ? 'primary' : 'ghost'} onClick={onNext}>
          {connected ? 'Continue' : 'Skip for now'}
        </Button>
      </div>
      {testEmail.isSuccess && (
        <p role="status" className="text-sm text-success">
          Test email queued. It appears in your Inbox in a few seconds.
        </p>
      )}
    </div>
  );
}

function MeetStep({ ws, onNext }: { ws: string; onNext: () => void }) {
  const registry = useAgentRegistry(ws);
  const patch = usePatchAgent(ws);
  const me = registry.data?.me;
  const team = (registry.data?.categories.flatMap((c) => c.agents) ?? []).filter((a) => a.id !== me?.id && a.status !== 'archived');
  return (
    <div className="flex flex-col gap-6">
      {registry.isLoading && <Skeleton className="h-40" />}
      <ErrorNote error={registry.error ?? patch.error} />
      {me && (
        <section aria-labelledby="meet-mine" className="flex flex-col gap-3">
          <h3 id="meet-mine" className="type-display text-xl tracking-[-0.01em]">
            Your personal agent
          </h3>
          <div className="flex items-center gap-4">
            <AgentFace avatar={resolveAvatar(me)} size="xl" />
            <div>
              <p className="font-semibold">{me.name}</p>
              <p className="text-sm text-ink-muted">{me.description}</p>
            </div>
          </div>
          <p className="text-sm text-ink-muted">Pick a face for it. You can change this later.</p>
          <AvatarPicker label="Choose an avatar for your personal agent" value={resolveAvatar(me)} disabled={patch.isPending} onChange={(stored) => patch.mutate({ id: me.id, avatar: stored })} />
        </section>
      )}
      {team.length > 0 && (
        <section aria-labelledby="meet-team" className="flex flex-col gap-3">
          <h3 id="meet-team" className="type-display text-xl tracking-[-0.01em]">
            The rest of the team
          </h3>
          <ul className="grid gap-3 sm:grid-cols-2">
            {team.map((a, i) => (
              <li key={a.id} className="rise letter flex items-center gap-3 rounded-xs p-3" style={riseStyle(i)}>
                <AgentFace avatar={resolveAvatar(a)} size="lg" index={i} />
                <div className="min-w-0">
                  <p className="font-semibold">{a.name}</p>
                  <p className="text-sm leading-snug text-ink-muted">{a.description}</p>
                </div>
              </li>
            ))}
          </ul>
        </section>
      )}
      <div>
        <Button onClick={onNext}>Looks good</Button>
      </div>
    </div>
  );
}

function WinStep({ ws }: { ws: string }) {
  const messages = useMessages(ws, {});
  const testEmail = useSendTestEmailEnsuringSource(ws);
  const count = messages.data?.pages.reduce((n, p) => n + p.items.length, 0) ?? 0;
  const linkClass =
    'inline-flex h-10 items-center rounded-sm border border-rule-strong bg-raised px-4 text-sm font-semibold hover:border-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent';
  return (
    <div className="flex flex-col gap-4">
      {messages.isLoading ? (
        <Skeleton className="h-16" />
      ) : count > 0 ? (
        <p className="type-display text-2xl leading-snug tracking-[-0.01em]">
          Meera has already read {count} {count === 1 ? 'email' : 'emails'}. Open one to see how it was sorted and what reply she drafted.
        </p>
      ) : (
        <>
          <p className="type-display text-2xl leading-snug tracking-[-0.01em]">Nothing in the inbox yet. Send yourself a test email to see the agents at work.</p>
          <div>
            <Button loading={testEmail.isPending} onClick={() => testEmail.mutate()}>
              Send a test email
            </Button>
          </div>
          {testEmail.isSuccess && (
            <p role="status" className="text-sm text-success">
              Test email queued. It appears in your Inbox in a few seconds.
            </p>
          )}
          <ErrorNote error={testEmail.error} />
        </>
      )}
      <div className="flex flex-wrap gap-2">
        <Link to={`/w/${ws}/inbox`} className={linkClass}>
          Open the Inbox
        </Link>
        <Link to={`/w/${ws}/agent-team`} className={linkClass}>
          See your team
        </Link>
      </div>
    </div>
  );
}

export function WelcomePage() {
  const { ws, workspace } = useWorkspace();
  const [step, setStep] = useState<Step>('connect');
  const index = STEPS.findIndex((s) => s.id === step);
  const goal = workspace?.goal?.trim();
  return (
    <Page>
      <PageHeader
        kicker="Welcome"
        title="Let us get your agents working"
        description={goal ? `Your goal: ${goal}` : 'Three quick steps and your team is ready.'}
        actions={<HelpPopover topic="welcome" />}
      />
      <ol aria-label="Progress" className="rise flex flex-wrap gap-x-6 gap-y-2" style={riseStyle(1)}>
        {STEPS.map((s, i) => (
          <li key={s.id} aria-current={i === index ? 'step' : undefined} className={cn('kicker border-b-2 pb-1', i === index ? 'border-accent text-ink' : 'border-transparent')}>
            {i + 1}. {s.label}
          </li>
        ))}
      </ol>
      <section aria-labelledby="welcome-step" className="rise flex max-w-3xl flex-col gap-4" style={riseStyle(2)}>
        <h2 id="welcome-step" className="type-display text-3xl leading-tight tracking-[-0.02em]">
          {STEPS[index]?.label}
        </h2>
        {step === 'connect' && <ConnectStep ws={ws} onNext={() => setStep('meet')} />}
        {step === 'meet' && <MeetStep ws={ws} onNext={() => setStep('win')} />}
        {step === 'win' && <WinStep ws={ws} />}
      </section>
    </Page>
  );
}
