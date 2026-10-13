import { useState } from 'react';
import { Link } from 'react-router';
import { Badge, Card, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { DraftPanel } from '../drafts/DraftPanel';
import { formatBytes, fullDate, humanize, relativeTime } from '../../lib/format';
import { ClassificationPanel } from './ClassificationPanel';
import { useMessage } from './hooks';
import { EmailBody } from './EmailBody';
import { Postmark } from './Postmark';
import { RouteTracker } from './RouteTracker';
import { riseStyle } from '../../lib/motion';
import { LabelStamp } from '../labels/LabelStamp';
import { SeeMatchesLink } from '../matching/SeeMatchesLink';
import { MemoryForm, MemoryItem } from '../memory/MemoryParts';

export function MessageDetailView({ ws, messageId }: { ws: string; messageId: string }) {
  const [awaitingDraft, setAwaitingDraft] = useState(false);
  const { data: m, isLoading, error } = useMessage(ws, messageId, awaitingDraft);

  if (isLoading) {
    return (
      <div className="flex justify-center p-12 text-ink-muted">
        <Spinner />
      </div>
    );
  }
  if (error || !m) {
    return (
      <div className="p-6">
        <ErrorNote error={error} />
      </div>
    );
  }

  return (
    <div className="grid gap-x-12 gap-y-8 px-8 pt-6 pb-8 2xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
      <div className="flex min-w-0 flex-col gap-8 2xl:col-start-1">
        <article className="rise flex flex-col gap-6" style={riseStyle(0)}>
          <header className="flex flex-col gap-5">
            <div className="flex items-start justify-between gap-6">
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-3">
                  {m.classification && <LabelStamp ws={ws} category={m.classification.category} size="lg" tilted />}
                  <StatusBadge status={m.processing_status} />
                </div>
                {(m.classification?.category === 'client_requirement' || m.classification?.category === 'candidate_referral') && (
                  <SeeMatchesLink ws={ws} messageId={m.id} category={m.classification.category} />
                )}
                <h2 className="type-display mt-4 text-4xl leading-[1.1] tracking-[-0.02em] break-words">{m.subject || '(no subject)'}</h2>
              </div>
              <Postmark receivedAt={m.received_at} />
            </div>
            <dl className="grid grid-cols-[auto_1fr] items-baseline gap-x-6 gap-y-1.5 text-sm">
              <dt className="kicker">From</dt>
              <dd className="font-mono text-[13px] break-all">{m.from}</dd>
              {m.original_from && (
                <>
                  <dt className="kicker">Original sender (unverified)</dt>
                  <dd className="font-mono text-[13px] break-all">{m.original_from}</dd>
                </>
              )}
              <dt className="kicker">To</dt>
              <dd className="font-mono text-[13px] break-all">{m.to.join(', ')}</dd>
              <dt className="kicker">Received</dt>
              <dd className="font-mono text-[13px]" title={fullDate(m.received_at)}>
                {fullDate(m.received_at)} ({relativeTime(m.received_at)})
              </dd>
            </dl>
            <RouteTracker status={m.processing_status} hasClassification={m.classification !== null} />
          </header>
          <div className="border-t border-rule-strong pt-6">
            <EmailBody text={m.body_text} html={m.body_html_safe} />
          </div>
        </article>

        {m.attachments.length > 0 && (
          <section className="rise border-t border-rule-strong pt-6" style={riseStyle(3)}>
            <h3 className="kicker">Attachments</h3>
            <ul className="mt-3 divide-y divide-rule text-sm">
              {m.attachments.map((a) => (
                <li key={a.id} className="flex flex-wrap items-center justify-between gap-2 py-2">
                  <span className="font-semibold break-all">{a.name}</span>
                  <span className="flex items-center gap-2 font-mono text-xs text-ink-muted">
                    {a.media_type ?? 'unknown type'} · {formatBytes(a.size_bytes)}
                    <Badge tone={a.scan_status === 'clean' ? 'success' : 'neutral'}>Scan: {a.scan_status}</Badge>
                  </span>
                </li>
              ))}
            </ul>
          </section>
        )}
      </div>

      <div className="flex min-w-0 flex-col gap-8 2xl:col-start-2 2xl:row-span-2 2xl:row-start-1">
        <div className="rise border-t border-rule-strong pt-6 2xl:border-t-0 2xl:pt-0" style={riseStyle(2)}>
          <ClassificationPanel ws={ws} messageId={m.id} classification={m.classification} />
        </div>
        <section className="rise flex flex-col gap-2 border-t border-rule-strong pt-6" style={riseStyle(3)}>
          <h3 className="kicker">Workflow</h3>
          {m.workflow_run ? (
            <div className="flex flex-col gap-1 text-sm">
              <div className="flex items-center gap-2">
                Run status: <StatusBadge status={m.workflow_run.status} />
              </div>
              {m.workflow_run.error && <p className="font-mono text-xs text-danger">{m.workflow_run.error}</p>}
            </div>
          ) : (
            <p className="text-sm text-ink-muted">No workflow run yet.</p>
          )}
          {m.review && (
            <p className="flex items-center gap-2 text-sm">
              <Link to={`/w/${ws}/reviews`} className="font-semibold text-accent underline underline-offset-2 hover:text-accent-hover">
                {humanize(m.review.kind)} review
              </Link>{' '}
              <StatusBadge status={m.review.status} />
            </p>
          )}
        </section>
        <section aria-labelledby="sender-heading" className="rise flex flex-col gap-3 border-t border-rule-strong pt-6" style={riseStyle(3)}>
          <div className="flex items-center justify-between gap-2">
            <h3 id="sender-heading" className="kicker">
              Sender
            </h3>
            <Link
              to={`/w/${ws}/memory/contacts/${encodeURIComponent(m.from)}`}
              className="text-sm text-accent underline underline-offset-2 hover:text-accent-hover"
            >
              Open profile
            </Link>
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
        <Card className="rise bg-surface p-6 shadow-[0_1px_0_var(--color-rule),0_12px_24px_-16px_rgba(28,27,24,0.25)]" style={riseStyle(4)}>
          <DraftPanel ws={ws} message={m} onAwaitingChange={setAwaitingDraft} />
        </Card>
      </div>

      <section className="rise min-w-0 border-t border-rule-strong pt-6 2xl:col-start-1" style={riseStyle(5)}>
        <h3 className="kicker">Audit trail</h3>
        {m.audit.length === 0 ? (
          <p className="mt-3 text-sm text-ink-muted">No events yet.</p>
        ) : (
          <ol className="mt-3 flex flex-col divide-y divide-rule text-sm">
            {m.audit.map((e) => (
              <li key={e.id} className="flex justify-between gap-3 py-1.5">
                <span>
                  <span className="font-semibold">{humanize(e.action.replace(/\./g, ' '))}</span>{' '}
                  <span className="text-ink-muted">
                    by {e.actor_type}
                    {e.actor_id ? ` (${e.actor_id})` : ''}
                  </span>
                </span>
                <time dateTime={e.created_at} className="shrink-0 font-mono text-xs text-ink-muted">
                  {fullDate(e.created_at)}
                </time>
              </li>
            ))}
          </ol>
        )}
      </section>
    </div>
  );
}
