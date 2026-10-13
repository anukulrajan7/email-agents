import { Link } from 'react-router';
import type { Match, MatchSource } from '../../api/types';
import { Button, Stamp } from '../../components/ui';
import { cn } from '../../lib/cn';
import { useDecideMatch } from './hooks';

export function experienceBand(min: number | null, max: number | null): string | undefined {
  if (min !== null && max !== null) return min === max ? `${min} yrs` : `${min}–${max} yrs`;
  if (min !== null) return `${min}+ yrs`;
  if (max !== null) return `Up to ${max} yrs`;
  return undefined;
}

export function noticeText(days: number | null): string | undefined {
  if (days === null) return undefined;
  return days === 0 ? 'Immediate joiner' : `${days}-day notice`;
}

export function Chip({ children }: { children: string }) {
  return <span className="rounded-xs border border-rule-strong bg-raised px-1.5 py-px text-xs text-ink">{children}</span>;
}

export function SkillChips({ skills, label }: { skills: string[]; label: string }) {
  if (skills.length === 0) return null;
  return (
    <ul aria-label={label} className="flex flex-wrap gap-1">
      {skills.map((s) => (
        <li key={s}>
          <Chip>{s}</Chip>
        </li>
      ))}
    </ul>
  );
}

export function SourceNote({ source }: { source: MatchSource }) {
  if (source !== 'rules') return null;
  return <p className="text-xs text-ink-muted">Extracted by keyword rules — check details</p>;
}

export function SourceEmailLink({ ws, messageId }: { ws: string; messageId: string }) {
  return (
    <Link to={`/w/${ws}/inbox/${messageId}`} className="text-[13px] text-accent underline underline-offset-2 hover:text-accent-hover">
      Source email
    </Link>
  );
}

const tier = (score: number) =>
  score >= 75
    ? { stroke: 'stroke-success', text: 'text-success' }
    : score >= 50
      ? { stroke: 'stroke-review', text: 'text-review-ink' }
      : { stroke: 'stroke-rule-strong', text: 'text-ink-muted' };

export function ScoreRing({ score, size = 64 }: { score: number; size?: number }) {
  const r = 26;
  const c = 2 * Math.PI * r;
  const pct = Math.max(0, Math.min(100, score));
  const t = tier(score);
  return (
    <div role="img" aria-label={`Score ${score} of 100`} className="relative shrink-0" style={{ width: size, height: size }}>
      <svg aria-hidden="true" viewBox="0 0 64 64" className="h-full w-full -rotate-90" fill="none" strokeWidth="6">
        <circle cx="32" cy="32" r={r} className="stroke-rule" />
        <circle
          cx="32"
          cy="32"
          r={r}
          className={t.stroke}
          strokeLinecap="round"
          strokeDasharray={c}
          strokeDashoffset={c * (1 - pct / 100)}
        />
      </svg>
      <span aria-hidden="true" className={cn('type-display absolute inset-0 flex items-center justify-center text-xl font-semibold', t.text)}>
        {score}
      </span>
    </div>
  );
}

export function ScoreBadge({ score }: { score: number }) {
  const t = tier(score);
  return (
    <span className={cn('font-mono text-xs font-semibold', t.text)} aria-label={`Top score ${score} of 100`}>
      Top {score}
    </span>
  );
}

export function MatchCard({ ws, match, showRequirement }: { ws: string; match: Match; showRequirement: boolean }) {
  const decide = useDecideMatch(ws);
  const { candidate: c, requirement: r } = match;
  const meta = [
    c.total_experience !== null ? `${c.total_experience} yrs` : undefined,
    c.location ?? undefined,
    noticeText(c.notice_days),
  ].filter((x): x is string => x !== undefined);
  const set = (status: Match['status']) => decide.mutate({ id: match.id, status });
  return (
    <article aria-label={`${c.name} for ${r.role_title}`} className="flex gap-4 rounded-sm border border-rule bg-surface p-4">
      <ScoreRing score={match.score} />
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="min-w-0">
            <h3 className="type-display text-xl tracking-[-0.01em] break-words">{c.name}</h3>
            {showRequirement && (
              <p className="text-sm text-ink-muted">
                for {r.role_title}
                {r.client ? ` · ${r.client}` : ''}
              </p>
            )}
            {meta.length > 0 && <p className="mt-0.5 text-sm text-ink-muted">{meta.join(' · ')}</p>}
          </div>
          {match.status === 'shortlisted' && <Stamp name="Shortlisted" color="sage" tilted />}
        </div>
        <ul aria-label="Why this match" className="flex flex-col gap-1 text-sm">
          {match.reasons.map((reason, i) => (
            <li key={i} className="flex items-start gap-2">
              <span aria-hidden="true" className={cn('w-4 shrink-0 font-bold', reason.ok ? 'text-success' : 'text-danger')}>
                {reason.ok ? '✓' : '✕'}
              </span>
              <span className="sr-only">{reason.ok ? 'Met: ' : 'Not met: '}</span>
              <span>{reason.text}</span>
            </li>
          ))}
        </ul>
        <div className="flex flex-wrap items-center gap-2">
          {match.status === 'suggested' ? (
            <>
              <Button size="sm" onClick={() => set('shortlisted')} disabled={decide.isPending} aria-label={`Shortlist ${c.name}`}>
                Shortlist
              </Button>
              <Button size="sm" variant="secondary" onClick={() => set('dismissed')} disabled={decide.isPending} aria-label={`Dismiss ${c.name}`}>
                Dismiss
              </Button>
            </>
          ) : (
            <Button size="sm" variant="secondary" onClick={() => set('suggested')} disabled={decide.isPending} aria-label={`Undo decision for ${c.name}`}>
              Undo
            </Button>
          )}
          {decide.isError && <span role="alert" className="text-sm text-danger">Could not save. Try again.</span>}
        </div>
      </div>
    </article>
  );
}
