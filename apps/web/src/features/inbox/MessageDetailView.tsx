import { useState } from 'react';
import { Card, RelativeTime, Skeleton, TextLink } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { DraftPanel } from '../drafts/DraftPanel';
import { humanize } from '../../lib/format';
import { ClassificationPanel } from './ClassificationPanel';
import { useMessage } from './hooks';
import { EmailReader } from './EmailReader';
import { riseStyle } from '../../lib/motion';
import { LabelStamp } from '../labels/LabelStamp';
import { MemoryForm, MemoryItem } from '../memory/MemoryParts';
import { StartAgentButton } from '../agents/StartAgentButton';

export function MessageDetailView({ ws, messageId }: { ws: string; messageId: string }) {
  const [awaitingDraft, setAwaitingDraft] = useState(false);
  const { data: m, isLoading, error } = useMessage(ws, messageId, awaitingDraft);

  if (isLoading) {
    return (
      <div role="status" aria-label="Loading email" aria-busy="true" className="flex max-w-3xl flex-col gap-5 px-4 pt-6 pb-8 sm:px-6 lg:px-8">
        <Skeleton className="h-7 w-3/4" />
        <Skeleton className="h-4 w-24" />
        <div className="flex items-center gap-3">
          <Skeleton className="h-10 w-10 rounded-full" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-3.5 w-1/3" />
            <Skeleton className="h-3 w-1/4" />
          </div>
        </div>
        <Skeleton className="h-24 w-full" />
        <div className="flex flex-col gap-2">
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-11/12" />
          <Skeleton className="h-3 w-4/5" />
        </div>
      </div>
    );
  }
  if (error || !m) {
    return (
      <div className="p-4 sm:p-6">
        <ErrorNote error={error ?? new Error('This email could not be found.')} />
      </div>
    );
  }

  return (
    <div className="grid gap-x-12 gap-y-8 px-4 pt-6 pb-10 sm:px-6 lg:px-8 2xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
      <div className="flex min-w-0 flex-col gap-8 2xl:col-start-1">
        <div className="rise" style={riseStyle(0)}>
          <EmailReader ws={ws} message={m} />
        </div>

        <Card className="rise bg-surface p-5 shadow-[0_1px_0_var(--color-rule),0_12px_24px_-16px_rgba(28,27,24,0.25)]" style={riseStyle(2)}>
          <DraftPanel ws={ws} message={m} onAwaitingChange={setAwaitingDraft} />
        </Card>
      </div>

      <div className="flex min-w-0 flex-col gap-8 2xl:col-start-2 2xl:row-span-2 2xl:row-start-1">
        <div className="rise border-t border-rule-strong pt-6 2xl:border-t-0 2xl:pt-0" style={riseStyle(2)}>
          <ClassificationPanel ws={ws} messageId={m.id} classification={m.classification} />
        </div>
        <section aria-labelledby="followup-heading" className="rise flex flex-col gap-3 border-t border-rule-strong pt-6" style={riseStyle(3)}>
          <h3 id="followup-heading" className="kicker">
            Follow-up
          </h3>
          {m.workflow_run ? (
            <div className="flex flex-col gap-1 text-sm">
              <p className="flex items-center gap-2">
                Automatic follow-up <StatusBadge status={m.workflow_run.status} />
              </p>
              {m.workflow_run.status === 'failed' && <p className="text-danger">Something went wrong with the follow-up. An admin can retry it from Workflows.</p>}
            </div>
          ) : (
            <p className="text-sm text-ink-muted">{m.processing_status === 'processed' ? 'Nothing else needed for this email.' : 'No automatic follow-up for this email.'}</p>
          )}
          {m.review && (
            <p className="flex items-center gap-2 text-sm">
              <TextLink to={`/w/${ws}/reviews`} className="font-semibold">
                {humanize(m.review.kind)} review
              </TextLink>{' '}
              <StatusBadge status={m.review.status} />
            </p>
          )}
          <StartAgentButton ws={ws} target={{ kind: 'inbox', message_id: m.id }} label="Let the agent handle it" />
        </section>
        <section aria-labelledby="sender-heading" className="rise flex flex-col gap-3 border-t border-rule-strong pt-6" style={riseStyle(3)}>
          <div className="flex items-center justify-between gap-2">
            <h3 id="sender-heading" className="kicker">
              Sender
            </h3>
            <TextLink to={`/w/${ws}/memory/contacts/${encodeURIComponent(m.from)}`} className="text-sm">
              Open profile
            </TextLink>
          </div>
          {m.contact ? (
            <div className="flex flex-col gap-1 text-sm">
              <p className="font-semibold break-words">{m.contact.display_name ?? m.contact.email}</p>
              <p className="flex flex-wrap items-center gap-2 text-ink-muted">
                {m.contact.message_count} {m.contact.message_count === 1 ? 'email' : 'emails'}
                {m.contact.top_categories.map((t) => (
                  <LabelStamp key={t.category} ws={ws} category={t.category} />
                ))}
              </p>
            </div>
          ) : (
            <p className="text-sm text-ink-muted">No profile yet.</p>
          )}
          {m.contact_memories.length > 0 && (
            <ul className="divide-y divide-rule border-y border-rule">
              {m.contact_memories.map((cm) => (
                <MemoryItem key={cm.id} ws={ws} memory={cm} />
              ))}
            </ul>
          )}
          <details className="text-sm">
            <summary className="cursor-pointer font-semibold text-ink-muted hover:text-ink focus-visible:outline-2 focus-visible:outline-accent">
              Remember something about this sender
            </summary>
            <div className="mt-3">
              <MemoryForm ws={ws} scope="contact" contactEmail={m.from} textareaLabel="What should we remember?" rows={2} />
            </div>
          </details>
        </section>
      </div>

      <section className="rise min-w-0 border-t border-rule-strong pt-6 2xl:col-start-1" style={riseStyle(5)}>
        <h3 className="kicker">History</h3>
        {m.audit.length === 0 ? (
          <p className="mt-3 text-sm text-ink-muted">No events yet.</p>
        ) : (
          <ol className="mt-3 flex flex-col divide-y divide-rule text-sm">
            {m.audit.map((e) => (
              <li key={e.id} className="flex justify-between gap-3 py-2">
                <span className="min-w-0 break-words">
                  <span className="font-semibold">{humanize(e.action.replace(/\./g, ' '))}</span>{' '}
                  <span className="text-ink-muted">{e.actor_type === 'system' ? 'automatically' : `by ${e.actor_id?.includes('@') ? e.actor_id : 'a teammate'}`}</span>
                </span>
                <RelativeTime iso={e.created_at} className="shrink-0 font-mono text-xs text-ink-muted" />
              </li>
            ))}
          </ol>
        )}
      </section>
    </div>
  );
}
