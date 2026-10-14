import { useLocation } from 'react-router';
import { Button } from '../../components/ui';
import { AgentFace } from '../agents-hub/AgentFace';
import { useGuide } from './GuideProvider';
import { pageOfPath } from './guides';

/** The explanation card at the top of a main page. Dismissed once, it stays dismissed for this person; the Guide page keeps every card. */
export function PageGuideCard() {
  const { guides, dismissed, dismiss } = useGuide();
  const page = pageOfPath(useLocation().pathname);
  const guide = guides.find((g) => g.page === page);
  if (!guide || dismissed.has(guide.id)) return null;
  return (
    <section
      aria-label={`About this page: ${guide.title}`}
      className="mx-auto w-full max-w-[1440px] shrink-0 px-4 pt-4 sm:px-6 lg:px-10"
    >
      <div className="letter flex flex-col gap-3 rounded-xs p-4 md:flex-row md:items-start md:gap-6">
        <AgentFace avatar={guide.avatar} size="md" className="hidden md:inline-flex" />
        <div className="grid min-w-0 flex-1 gap-4 text-sm leading-snug md:grid-cols-3">
          <div>
            <p className="kicker">What this page does</p>
            <p className="mt-1 text-ink">{guide.what}</p>
          </div>
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
        <Button variant="ghost" size="sm" className="self-start" aria-label={`Dismiss: ${guide.title}`} onClick={() => dismiss(guide.id)}>
          Got it
        </Button>
      </div>
    </section>
  );
}
