import { useEffect, useId, useRef, useState, type CSSProperties } from 'react';
import { Button } from '../../components/ui';
import { cn } from '../../lib/cn';
import { useReducedMotion } from '../../lib/useReducedMotion';
import { stepAnchor, type Guide } from './guides';

type Box = { top: number; left: number; width: number; height: number };
type Found = { status: 'searching' } | { status: 'missing' } | { status: 'found'; box: Box };

const PAD = 6;
const GAP = 12;
const MISSING_AFTER_MS = 1200;
const POLL_MS = 100;
const CALLOUT_ROOM = 260;
const FOCUSABLE = 'button:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';

function measure(anchor: string): Box | undefined {
  const el = document.querySelector<HTMLElement>(`[data-guide="${anchor}"]`);
  if (!el) return undefined;
  const r = el.getBoundingClientRect();
  if (r.width <= 0 || r.height <= 0) return undefined;
  const top = Math.max(r.top, 0);
  const left = Math.max(r.left, 0);
  const bottom = Math.min(r.bottom, window.innerHeight);
  const right = Math.min(r.right, window.innerWidth);
  if (bottom <= top || right <= left) return undefined;
  return { top, left, width: right - left, height: bottom - top };
}

function useAnchor(anchor: string, smooth: boolean): Found {
  const [found, setFound] = useState<Found>({ status: 'searching' });

  useEffect(() => {
    setFound({ status: 'searching' });
    let located = false;
    const started = Date.now();
    const update = () => {
      const box = measure(anchor);
      if (box) setFound({ status: 'found', box });
      return box !== undefined;
    };
    const poll = window.setInterval(() => {
      if (located) return;
      const el = document.querySelector<HTMLElement>(`[data-guide="${anchor}"]`);
      if (el && update()) {
        located = true;
        el.scrollIntoView?.({ block: 'center', inline: 'nearest', behavior: smooth ? 'smooth' : 'auto' });
        window.clearInterval(poll);
      } else if (Date.now() - started > MISSING_AFTER_MS) {
        setFound((f) => (f.status === 'searching' ? { status: 'missing' } : f));
      }
    }, POLL_MS);
    const onMove = () => {
      if (located && !update()) {
        located = false;
        setFound({ status: 'missing' });
      }
    };
    window.addEventListener('resize', onMove);
    window.addEventListener('scroll', onMove, true);
    return () => {
      window.clearInterval(poll);
      window.removeEventListener('resize', onMove);
      window.removeEventListener('scroll', onMove, true);
    };
  }, [anchor, smooth]);

  return found;
}

function calloutPosition(box: Box): CSSProperties {
  const vw = window.innerWidth;
  const vh = window.innerHeight;
  const width = Math.min(352, vw - 32);
  const left = Math.min(Math.max(box.left - PAD, 16), Math.max(vw - width - 16, 16));
  const below = vh - (box.top + box.height + PAD);
  const above = box.top - PAD;
  if (below >= CALLOUT_ROOM) return { left, width, top: box.top + box.height + PAD + GAP };
  if (above >= CALLOUT_ROOM) return { left, width, bottom: vh - box.top + PAD + GAP };
  return { left, width, bottom: 16 };
}

export function Spotlight({
  guide,
  step,
  hasNextGuide,
  onStep,
  onClose,
}: {
  guide: Guide;
  step: number;
  hasNextGuide: boolean;
  onStep: (next: number) => void;
  onClose: (finished: boolean) => void;
}) {
  const reduced = useReducedMotion();
  const current = guide.steps[step];
  const found = useAnchor(current ? stepAnchor(guide, current) : guide.anchor, !reduced);
  const callout = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const last = step === guide.steps.length - 1;
  const total = guide.steps.length;

  const latest = useRef({ step, last, onStep, onClose });
  latest.current = { step, last, onStep, onClose };

  useEffect(() => {
    callout.current?.focus({ preventScroll: true });
  }, [step, guide.id]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const { step: s, last: isLast, onStep: go, onClose: close } = latest.current;
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        close(false);
      } else if (e.key === 'ArrowRight') {
        e.preventDefault();
        if (isLast) close(true);
        else go(s + 1);
      } else if (e.key === 'ArrowLeft') {
        e.preventDefault();
        if (s > 0) go(s - 1);
      } else if (e.key === 'Tab' && callout.current) {
        const items = Array.from(callout.current.querySelectorAll<HTMLElement>(FOCUSABLE));
        const first = items[0];
        const end = items[items.length - 1];
        if (!first || !end) return;
        const active = document.activeElement;
        const inside = callout.current.contains(active);
        if (!inside || (!e.shiftKey && active === end) || (e.shiftKey && (active === first || active === callout.current))) {
          e.preventDefault();
          (e.shiftKey && inside ? end : first).focus();
        }
      }
    };
    document.addEventListener('keydown', onKeyDown, true);
    return () => document.removeEventListener('keydown', onKeyDown, true);
  }, []);

  if (!current) return null;

  const box = found.status === 'found' ? found.box : undefined;
  const style: CSSProperties = box
    ? calloutPosition(box)
    : { left: '50%', top: '50%', width: Math.min(352, window.innerWidth - 32), transform: 'translate(-50%, -50%)' };

  return (
    <div data-testid="spotlight" data-motion={reduced ? 'still' : 'smooth'} data-target={box ? 'found' : found.status}>
      <div aria-hidden="true" className="fixed inset-0 z-[60]" />
      {box ? (
        <div
          aria-hidden="true"
          data-testid="spotlight-cutout"
          className={cn(
            'pointer-events-none fixed z-[60] rounded-sm outline-2 outline-offset-0 outline-accent',
            !reduced && 'transition-[top,left,width,height] duration-200 ease-out',
          )}
          style={{
            top: box.top - PAD,
            left: box.left - PAD,
            width: box.width + PAD * 2,
            height: box.height + PAD * 2,
            boxShadow: '0 0 0 100vmax rgba(28, 27, 24, 0.6)',
          }}
        />
      ) : (
        <div aria-hidden="true" className="pointer-events-none fixed inset-0 z-[60] bg-ink/60" />
      )}
      <div
        ref={callout}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        className="fixed z-[61] flex flex-col gap-3 rounded-sm border border-ink bg-raised p-4 shadow-[0_18px_32px_-20px_rgba(28,27,24,0.5)] outline-none"
        style={style}
      >
        <p className="kicker">
          Step {step + 1} of {total}
        </p>
        <h2 id={titleId} className="type-display text-xl leading-tight tracking-[-0.01em]">
          {guide.title}
        </h2>
        <p className="text-sm leading-relaxed text-ink">{current.text}</p>
        {!box && found.status === 'missing' && (
          <p className="text-[13px] leading-snug text-ink-muted">
            We cannot point at this part of the screen right now. It may be hidden at this window size or still loading.
          </p>
        )}
        {last && guide.safe && (
          <p className="border-l-[3px] border-success bg-success-tint px-3 py-2 text-[13px] leading-snug text-ink">
            <span className="font-semibold">Why it is safe. </span>
            {guide.safe}
          </p>
        )}
        <div className="flex items-center justify-between gap-2">
          <Button variant="ghost" size="sm" onClick={() => onClose(false)}>
            Close
          </Button>
          <div className="flex gap-2">
            {step > 0 && (
              <Button variant="secondary" size="sm" onClick={() => onStep(step - 1)}>
                Back
              </Button>
            )}
            <Button size="sm" onClick={() => (last ? onClose(true) : onStep(step + 1))}>
              {last ? (hasNextGuide ? 'Next guide' : 'Done') : 'Next'}
            </Button>
          </div>
        </div>
      </div>
      <p role="status" aria-live="polite" className="sr-only">
        Step {step + 1} of {total}: {guide.title}. {current.text}
      </p>
    </div>
  );
}
