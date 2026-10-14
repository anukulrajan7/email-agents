import { Badge, Button, Card, CardTitle, EmptyState, SkeletonList } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { humanize, relativeTime } from '../../lib/format';
import { useDecideSubmission, useSubmissions } from './hooks';
import type { LinkOption, Submission } from './types';

const OUTCOME_TONE = { pending: 'neutral', interview: 'info', selected: 'success', joined: 'success', rejected: 'danger' } as const;

function Choices({ title, options, onPick, disabled }: { title: string; options: LinkOption[]; onPick: (id: string) => void; disabled: boolean }) {
  if (options.length === 0) return null;
  return (
    <div className="flex flex-wrap items-center gap-2">
      <span className="text-sm text-ink-muted">{title}</span>
      {options.map((o) => (
        <Button key={o.id} size="sm" variant="secondary" disabled={disabled} onClick={() => onPick(o.id)}>
          {o.name ?? o.role_title}
          {o.client ? ` · ${o.client}` : ''}
        </Button>
      ))}
    </div>
  );
}

function Review({ s, ws }: { s: Submission; ws: string }) {
  const decide = useDecideSubmission(ws);
  return (
    <li className="flex flex-col gap-2 border-b border-rule py-3 last:border-b-0">
      <p className="text-[15px] text-ink">
        <strong>{s.person_name}</strong> sent <strong>{s.candidate_name ?? s.attachment_name}</strong> to {s.client_domain} {relativeTime(s.sent_at)}
      </p>
      <Choices
        title="Which candidate?"
        options={s.candidate_id ? [] : (s.options.candidates ?? [])}
        disabled={decide.isPending}
        onPick={(id) => decide.mutate({ id: s.id, body: { candidate_id: id } })}
      />
      <Choices
        title="Which requirement?"
        options={s.requirement_id ? [] : (s.options.requirements ?? [])}
        disabled={decide.isPending}
        onPick={(id) => decide.mutate({ id: s.id, body: { requirement_id: id } })}
      />
      <div>
        <Button size="sm" variant="ghost" disabled={decide.isPending} onClick={() => decide.mutate({ id: s.id, body: { dismissed: true } })}>
          Not a submission
        </Button>
      </div>
      <ErrorNote error={decide.error} />
    </li>
  );
}

/** Detected submissions: the ones a person must confirm, then the latest with their outcome. */
export function SubmissionsPanel({ ws }: { ws: string }) {
  const all = useSubmissions(ws);
  const items = all.data?.items ?? [];
  const review = items.filter((s) => s.link_status === 'needs_confirm');

  return (
    <section aria-label="Submissions" className="flex flex-col gap-3">
      <CardTitle>Submissions</CardTitle>
      <p className="max-w-[62ch] text-sm text-ink-muted">
        A submission is a mail a recruiter sent to a client with a CV attached. Each one is linked to its candidate and requirement; unclear links wait here for you.
      </p>
      <ErrorNote error={all.error} />
      {all.isLoading && <SkeletonList rows={2} label="Loading submissions" />}
      {all.data && items.length === 0 && (
        <Card>
          <EmptyState
            title="No submissions yet"
            description="Connect each recruiter's mailbox in Integrations, add them in People with their email, and a CV sent to a client shows up here within 15 minutes."
          />
        </Card>
      )}
      {review.length > 0 && (
        <Card>
          <p className="kicker mb-1">Needs your confirmation ({review.length})</p>
          <ul>
            {review.map((s) => (
              <Review key={s.id} s={s} ws={ws} />
            ))}
          </ul>
        </Card>
      )}
      {items.length > 0 && (
        <ul className="flex flex-col divide-y divide-rule rounded-sm border border-rule bg-surface">
          {items.slice(0, 20).map((s) => (
            <li key={s.id} className="flex flex-wrap items-center justify-between gap-2 px-3 py-2 text-sm">
              <span className="min-w-0">
                <strong>{s.candidate_name ?? s.attachment_name}</strong> → {s.client ?? s.client_domain}
                {s.requirement_title ? ` · ${s.requirement_title}` : ''} <span className="text-ink-muted">by {s.person_name}</span>
              </span>
              <Badge tone={OUTCOME_TONE[s.outcome]}>{humanize(s.outcome)}</Badge>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
