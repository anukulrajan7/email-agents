import { Button, Page, PageHeader } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { AgentFace } from '../agents-hub/AgentFace';
import { useGuide } from './GuideProvider';
import type { Guide } from './guides';

function GuideCard({ guide, index, tried, featured }: { guide: Guide; index: number; tried: boolean; featured?: boolean }) {
  const { start, setTried } = useGuide();
  return (
    <article
      aria-labelledby={`guide-${guide.id}`}
      data-testid={`guide-card-${guide.id}`}
      className="rise letter flex h-full flex-col gap-4 rounded-xs p-5"
      style={riseStyle(index + 1)}
    >
      <div className="flex items-start gap-4">
        <AgentFace avatar={guide.avatar} size={featured ? 'lg' : 'md'} index={index} />
        <div className="min-w-0 flex-1">
          <h3 id={`guide-${guide.id}`} className="type-display text-xl leading-tight tracking-[-0.01em] break-words">
            {guide.title}
          </h3>
          <p className="mt-1 text-sm leading-snug text-ink-muted">{guide.what}</p>
        </div>
      </div>
      <div>
        <p className="kicker">Try it</p>
        <ol className="mt-2 flex list-decimal flex-col gap-1.5 pl-5 text-sm leading-snug text-ink marker:font-mono marker:text-xs marker:text-ink-muted">
          {guide.steps.map((s) => (
            <li key={s.text}>{s.text}</li>
          ))}
        </ol>
      </div>
      {guide.safe && (
        <p className="border-l-[3px] border-success bg-success-tint px-3 py-2 text-[13px] leading-snug text-ink">
          <span className="font-semibold">Why it is safe. </span>
          {guide.safe}
        </p>
      )}
      <div className="mt-auto flex flex-wrap items-center justify-between gap-2 border-t border-rule pt-3">
        <button
          type="button"
          aria-pressed={tried}
          aria-label={`${guide.title}: ${tried ? 'tried' : 'not tried yet'}`}
          onClick={() => setTried(guide.id, !tried)}
          className="inline-flex h-8 items-center gap-2 rounded-sm px-1 text-[13px] font-semibold text-ink-muted hover:text-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
        >
          <span
            aria-hidden="true"
            className={
              tried
                ? 'flex h-5 w-5 items-center justify-center rounded-full bg-success text-xs text-accent-ink'
                : 'flex h-5 w-5 items-center justify-center rounded-full border border-rule-strong text-xs'
            }
          >
            {tried ? '✓' : ''}
          </span>
          {tried ? 'Tried' : 'Not tried yet'}
        </button>
        <Button size="sm" variant={tried ? 'secondary' : 'primary'} aria-label={`Show me: ${guide.title}`} onClick={() => start(guide.id)}>
          Show me
        </Button>
      </div>
    </article>
  );
}

export function GuidePage() {
  const { guides, startHere, tried } = useGuide();
  const startIds = new Set(startHere.map((g) => g.id));
  const others = guides.filter((g) => !startIds.has(g.id));
  const done = guides.filter((g) => tried.has(g.id)).length;
  const showStart = startHere.some((g) => !tried.has(g.id));
  let index = 0;

  return (
    <Page>
      <PageHeader
        kicker="Learn by doing"
        title="Guide"
        description="Short cards that explain each part of Post Room. Press Show me and we point at the real button on the real page."
        actions={
          <p className="font-mono text-xs text-ink-muted" role="status">
            {done} of {guides.length} tried
          </p>
        }
      />
      {showStart && (
        <section aria-labelledby="guide-start" className="flex flex-col gap-4">
          <div>
            <h2 id="guide-start" className="type-display text-2xl tracking-[-0.01em]">
              Start here
            </h2>
            <p className="text-sm text-ink-muted">If you are new, these three cover the basics.</p>
          </div>
          <ul className="grid auto-rows-fr grid-cols-1 gap-4 lg:grid-cols-3">
            {startHere.map((g) => (
              <li key={g.id}>
                <GuideCard guide={g} index={index++} tried={tried.has(g.id)} featured />
              </li>
            ))}
          </ul>
        </section>
      )}
      <section aria-labelledby="guide-all" className="flex flex-col gap-4">
        <h2 id="guide-all" className="type-display text-2xl tracking-[-0.01em]">
          {showStart ? 'More guides' : 'All guides'}
        </h2>
        <ul className="grid auto-rows-fr grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
          {(showStart ? others : guides).map((g) => (
            <li key={g.id}>
              <GuideCard guide={g} index={index++} tried={tried.has(g.id)} />
            </li>
          ))}
        </ul>
      </section>
    </Page>
  );
}
