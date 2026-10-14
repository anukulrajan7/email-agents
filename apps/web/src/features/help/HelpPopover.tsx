import { useEffect, useId, useRef, useState } from 'react';
import { cn } from '../../lib/cn';
import { HELP, type HelpTopic } from './content';

export function HelpPopover({ topic, label = 'What is this?', className }: { topic: HelpTopic; label?: string; className?: string }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const root = useRef<HTMLSpanElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const entry = HELP[topic];

  useEffect(() => {
    if (!open) return;
    const onPointer = (e: PointerEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  return (
    <span
      ref={root}
      className={cn('relative inline-block', className)}
      onKeyDown={(e) => {
        if (e.key === 'Escape' && open) {
          e.stopPropagation();
          setOpen(false);
          trigger.current?.focus();
        }
      }}
    >
      <button
        ref={trigger}
        type="button"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen((v) => !v)}
        className={cn(
          'inline-flex h-8 items-center gap-2 rounded-sm border border-rule-strong bg-raised px-3 text-[13px] font-semibold text-ink',
          'hover:border-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
        )}
      >
        <span aria-hidden="true" className="flex h-4 w-4 items-center justify-center rounded-full bg-ink font-mono text-[10px] text-accent-ink">
          ?
        </span>
        {label}
      </button>
      {open && (
        <div
          id={id}
          role="region"
          aria-label={entry.title}
          className="rise absolute right-0 z-30 mt-2 flex w-[min(22rem,calc(100vw-2rem))] flex-col gap-3 rounded-sm border border-ink bg-raised p-4 text-left shadow-[0_18px_32px_-20px_rgba(28,27,24,0.5)]"
        >
          <p className="type-display text-lg leading-tight tracking-[-0.01em]">{entry.title}</p>
          {entry.body.map((p) => (
            <p key={p} className="text-sm leading-relaxed text-ink-muted">
              {p}
            </p>
          ))}
          {entry.points && (
            <dl className="flex flex-col gap-2 border-t border-rule pt-3 text-sm">
              {entry.points.map((pt) => (
                <div key={pt.term}>
                  <dt className="font-semibold text-ink">{pt.term}</dt>
                  <dd className="text-ink-muted">{pt.text}</dd>
                </div>
              ))}
            </dl>
          )}
        </div>
      )}
    </span>
  );
}
