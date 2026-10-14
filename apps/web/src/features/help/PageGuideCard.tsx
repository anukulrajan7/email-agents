import { useId, useState } from 'react';
import { useLocation } from 'react-router';
import { Button } from '../../components/ui';
import { AgentFace } from '../agents-hub/AgentFace';
import { useGuide } from './GuideProvider';
import { pageOfPath } from './guides';

/** The explanation card at the top of a main page: one line, with the steps behind "How to use". Dismissed once, it stays dismissed for this person; the Guide page keeps every card. */
export function PageGuideCard() {
  const { guides, dismissed, dismiss } = useGuide();
  const [open, setOpen] = useState(false);
  const detailsId = useId();
  const page = pageOfPath(useLocation().pathname);
  const guide = guides.find((g) => g.page === page);
  if (!guide || dismissed.has(guide.id)) return null;
  return (
    <section
      aria-label={`About this page: ${guide.title}`}
      className="mx-auto w-full max-w-[1440px] shrink-0 px-4 pt-3 sm:px-6 lg:px-10"
    >
      <div className="letter flex flex-col gap-2 rounded-xs px-4 py-2">
        <div className="flex items-center gap-3">
          <AgentFace avatar={guide.avatar} size="sm" className="hidden md:inline-flex" />
          <p className="min-w-0 flex-1 text-sm leading-snug">
            <span className="kicker mr-2">What this page does</span>
            <span className="text-ink">{guide.what}</span>
          </p>
          <Button variant="ghost" size="sm" aria-expanded={open} aria-controls={detailsId} onClick={() => setOpen((o) => !o)}>
            How to use
          </Button>
          <Button variant="ghost" size="sm" aria-label={`Dismiss: ${guide.title}`} onClick={() => dismiss(guide.id)}>
            Got it
          </Button>
        </div>
        {open && (
          <div id={detailsId} className="grid gap-3 border-t border-rule pt-2 text-sm leading-snug md:grid-cols-2">
            <div>
              <p className="kicker">How to use it</p>
              <ol className="mt-1 flex list-decimal flex-col gap-1 pl-5 text-ink marker:font-mono marker:text-xs marker:text-ink-muted">
                {guide.steps.slice(0, 3).map((s) => (
                  <li key={s}>{s}</li>
                ))}
              </ol>
            </div>
            <div>
              <p className="kicker">Why it is safe</p>
              <p className="mt-1 text-ink">{guide.safe ?? 'You stay in control. Nothing is sent or changed without you.'}</p>
            </div>
          </div>
        )}
      </div>
    </section>
  );
}
