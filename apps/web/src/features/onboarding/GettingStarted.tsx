import { useState } from 'react';
import { Link } from 'react-router';
import { Button, Card, CardTitle } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { riseStyle } from '../../lib/motion';
import { useMessages } from '../inbox/hooks';
import { useSendTestEmailEnsuringSource, useSources } from '../sources/hooks';
import { ReplySetup } from './ReplySetup';
import { dismiss, hasVisitedLabels, isDismissed } from './storage';

type Step = { title: string; why: string; done: boolean; to: string; cta: string; action?: 'test-email' };

export function GettingStarted({ ws }: { ws: string }) {
  const [hidden, setHidden] = useState(() => isDismissed(ws));
  const messages = useMessages(ws, {});
  const sources = useSources(ws);
  const testEmail = useSendTestEmailEnsuringSource(ws);

  const gmailConnected = sources.data?.items.some((s) => s.kind === 'nango_gmail' && s.status === 'connected') ?? false;
  const replySetup = gmailConnected ? <ReplySetup ws={ws} /> : null;

  if (hidden || !messages.data) return replySetup;
  if (messages.data.pages.some((p) => p.items.length > 0)) return replySetup;

  const steps: Step[] = [
    {
      title: 'Connect email',
      why: 'The AI can only sort mail it can see, so link a mailbox or get a forwarding address.',
      done: sources.data?.items.some((s) => s.status === 'connected') ?? false,
      to: `/w/${ws}/integrations`,
      cta: 'Open Integrations',
    },
    {
      title: 'Review your labels',
      why: 'Labels are the categories the AI sorts into — check they match how your team works.',
      done: hasVisitedLabels(ws),
      to: `/w/${ws}/labels`,
      cta: 'Open Labels',
    },
    {
      title: 'Send a test email or wait for mail',
      why: 'Your first message shows how it gets labelled and what reply the AI drafts.',
      done: false,
      to: `/w/${ws}/integrations`,
      cta: 'Send a test email',
      action: 'test-email',
    },
    {
      title: 'Teach it',
      why: 'Add notes about your team and senders so replies sound like you and stay accurate.',
      done: false,
      to: `/w/${ws}/memory`,
      cta: 'Open Memory',
    },
  ];

  return (
    <>
    {replySetup}
    <Card className="rise m-6 max-w-2xl bg-raised" style={riseStyle(0)} role="region" aria-labelledby="getting-started-heading">
      <div className="flex items-start justify-between gap-4">
        <div>
          <p className="kicker">New here?</p>
          <CardTitle id="getting-started-heading">Getting started</CardTitle>
        </div>
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            dismiss(ws);
            setHidden(true);
          }}
        >
          Dismiss
        </Button>
      </div>
      <ol className="mt-4 flex flex-col divide-y divide-rule">
        {steps.map((s, i) => (
          <li key={s.title} className="flex items-start gap-3 py-3">
            <span
              aria-hidden="true"
              className={cn(
                'mt-0.5 flex h-6 w-6 shrink-0 items-center justify-center rounded-full border text-xs font-semibold',
                s.done ? 'border-success bg-success text-accent-ink' : 'border-rule-strong text-ink-muted',
              )}
            >
              {s.done ? '✓' : i + 1}
            </span>
            <div className="min-w-0 flex-1">
              <p className="text-sm font-semibold text-ink">
                {s.title}
                {s.done && <span className="sr-only"> (done)</span>}
              </p>
              <p className="text-sm text-ink-muted">{s.why}</p>
              {s.action === 'test-email' ? (
                <div className="mt-1 flex flex-col items-start gap-1">
                  <Button variant="secondary" size="sm" onClick={() => testEmail.mutate()} loading={testEmail.isPending}>
                    {s.cta}
                  </Button>
                  {testEmail.isSuccess && (
                    <p role="status" className="text-sm text-success">
                      Test email queued — it appears in the Inbox in a few seconds
                    </p>
                  )}
                  <ErrorNote error={testEmail.error} />
                </div>
              ) : (
                <Link to={s.to} className="text-sm font-semibold text-accent underline underline-offset-2 hover:text-accent-hover">
                  {s.cta}
                </Link>
              )}
            </div>
          </li>
        ))}
      </ol>
    </Card>
    </>
  );
}
