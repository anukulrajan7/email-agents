import { useEffect, useRef, useState } from 'react';
import { useLocation } from 'react-router';
import { TextLink, Button } from '../../components/ui';
import { useGuide } from './GuideProvider';
import { pageOfPath } from './guides';
import { readNudged, writeNudged } from './guideStorage';
import { isTourDone, markTourDone } from './tourStorage';

const CARD =
  'rise fixed bottom-4 left-4 z-40 flex w-[min(21rem,calc(100vw-2rem))] flex-col gap-3 rounded-sm border border-ink bg-raised p-4 shadow-[0_18px_32px_-20px_rgba(28,27,24,0.5)]';

function WelcomeCard({ onDone }: { onDone: () => void }) {
  const { ws, startHere, start } = useGuide();
  const [first, ...rest] = startHere;
  if (!first) return null;
  return (
    <section role="region" aria-label="Quick tour" className={CARD}>
      <p className="kicker">Welcome</p>
      <p className="type-display text-xl leading-tight tracking-[-0.01em]">Three things to try first</p>
      <ol className="flex flex-col gap-1.5 text-sm text-ink-muted">
        {startHere.map((g, i) => (
          <li key={g.id}>
            <span className="font-mono text-xs text-ink">{i + 1}.</span> <span className="font-semibold text-ink">{g.title}</span>
          </li>
        ))}
      </ol>
      <p className="text-sm leading-relaxed text-ink-muted">We point at the real buttons, one step at a time. Nothing changes while you look.</p>
      <div className="flex items-center justify-between gap-2">
        <Button variant="ghost" size="sm" onClick={onDone}>
          Skip tour
        </Button>
        <div className="flex items-center gap-3">
          <TextLink to={`/w/${ws}/guide`} className="text-[13px] font-semibold" onClick={onDone}>
            All guides
          </TextLink>
          <Button
            size="sm"
            onClick={() => {
              onDone();
              start(first.id, rest.map((g) => g.id));
            }}
          >
            Show me around
          </Button>
        </div>
      </div>
    </section>
  );
}

function Nudge() {
  const { ws, user, guides, tried, running, start } = useGuide();
  const location = useLocation();
  const page = pageOfPath(location.pathname);
  const [shown, setShown] = useState<string>();
  const guide = guides.find((g) => g.nudge && g.page === page);

  const seen = useRef<string>(undefined);

  useEffect(() => {
    if (seen.current !== page) {
      seen.current = undefined;
      setShown(undefined);
    }
  }, [page]);

  useEffect(() => {
    if (!guide || tried.has(guide.id)) return;
    if (seen.current === page) {
      setShown(page);
      return;
    }
    const nudged = readNudged(user, ws);
    if (nudged.has(page)) return;
    nudged.add(page);
    writeNudged(user, ws, nudged);
    seen.current = page;
    setShown(page);
  }, [guide, page, tried, user, ws]);

  if (!guide || shown !== page || running || tried.has(guide.id)) return null;
  return (
    <section role="region" aria-label="New here?" className={CARD}>
      <p className="kicker">New here?</p>
      <p className="type-display text-lg leading-tight tracking-[-0.01em]">See how {guide.topic} works</p>
      <p className="text-sm leading-relaxed text-ink-muted">{guide.what}</p>
      <div className="flex justify-end gap-2">
        <Button
          variant="ghost"
          size="sm"
          onClick={() => {
            seen.current = undefined;
            setShown(undefined);
          }}
        >
          Not now
        </Button>
        <Button
          size="sm"
          onClick={() => {
            seen.current = undefined;
            setShown(undefined);
            start(guide.id);
          }}
        >
          Show me
        </Button>
      </div>
    </section>
  );
}

/** One system for first-run help: the welcome card first, then a one-time nudge on each page. */
export function GuideLauncher() {
  const { running } = useGuide();
  const location = useLocation();
  const page = pageOfPath(location.pathname);
  const [welcomed, setWelcomed] = useState(isTourDone);
  const quiet = page === 'welcome' || page === 'guide';

  if (running || quiet) return null;
  if (!welcomed) {
    return (
      <WelcomeCard
        onDone={() => {
          markTourDone();
          setWelcomed(true);
        }}
      />
    );
  }
  return <Nudge />;
}
