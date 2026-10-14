import { Page, PageHeader, TextLink } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { AgentFace } from '../agents-hub/AgentFace';
import { useGuide } from './GuideProvider';
import type { Guide } from './guides';

function GuideCard({ guide, index, ws }: { guide: Guide; index: number; ws: string }) {
  return (
    <article
      aria-labelledby={`guide-${guide.id}`}
      data-testid={`guide-card-${guide.id}`}
      className="rise letter flex h-full flex-col gap-4 rounded-xs p-5"
      style={riseStyle(index + 1)}
    >
      <div className="flex items-start gap-4">
        <AgentFace avatar={guide.avatar} size="md" index={index} />
        <div className="min-w-0 flex-1">
          <h3 id={`guide-${guide.id}`} className="type-display text-xl leading-tight tracking-[-0.01em] break-words">
            {guide.title}
          </h3>
          <p className="mt-1 text-sm leading-snug text-ink-muted">{guide.what}</p>
        </div>
      </div>
      <div>
        <p className="kicker">How to use it</p>
        <ol className="mt-2 flex list-decimal flex-col gap-1.5 pl-5 text-sm leading-snug text-ink marker:font-mono marker:text-xs marker:text-ink-muted">
          {guide.steps.map((s) => (
            <li key={s}>{s}</li>
          ))}
        </ol>
      </div>
      {guide.safe && (
        <p className="border-l-[3px] border-success bg-success-tint px-3 py-2 text-[13px] leading-snug text-ink">
          <span className="font-semibold">Why it is safe. </span>
          {guide.safe}
        </p>
      )}
      <div className="mt-auto border-t border-rule pt-3">
        <TextLink to={`/w/${ws}/${guide.page}`} aria-label={`Go to page: ${guide.title}`} className="text-[13px] font-semibold">
          Go to page
        </TextLink>
      </div>
    </article>
  );
}

export function GuidePage() {
  const { ws, guides, dismissed, restoreAll } = useGuide();
  return (
    <Page>
      <PageHeader
        kicker="Learn the basics"
        title="Guide"
        description="A short card for each part of Post Room: what it does, how to use it and why it is safe."
        actions={
          dismissed.size > 0 ? (
            <button
              type="button"
              onClick={restoreAll}
              className="inline-flex h-8 items-center rounded-sm px-2 text-[13px] font-semibold text-accent underline underline-offset-[3px] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
            >
              Show the page cards again
            </button>
          ) : undefined
        }
      />
      <ul className="grid auto-rows-fr grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
        {guides.map((g, i) => (
          <li key={g.id}>
            <GuideCard guide={g} index={i} ws={ws} />
          </li>
        ))}
      </ul>
    </Page>
  );
}
