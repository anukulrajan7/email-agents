import { useId, type KeyboardEvent, type ReactNode } from 'react';
import { cn } from '../../lib/cn';

export type TabItem<T extends string> = { id: T; label: string; count?: number };

export function Tabs<T extends string>({
  label,
  tabs,
  value,
  onChange,
  children,
}: {
  label: string;
  tabs: TabItem<T>[];
  value: T;
  onChange: (id: T) => void;
  children: ReactNode;
}) {
  const uid = useId();

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const index = tabs.findIndex((t) => t.id === value);
    const target =
      e.key === 'ArrowRight' ? index + 1 : e.key === 'ArrowLeft' ? index - 1 : e.key === 'Home' ? 0 : e.key === 'End' ? tabs.length - 1 : undefined;
    if (target === undefined) return;
    e.preventDefault();
    const next = tabs[(target + tabs.length) % tabs.length];
    if (!next) return;
    onChange(next.id);
    document.getElementById(`${uid}-tab-${next.id}`)?.focus();
  };

  return (
    <div className="flex flex-col gap-5">
      <div role="tablist" aria-label={label} onKeyDown={onKeyDown} className="flex gap-1 overflow-x-auto border-b border-rule [scrollbar-width:none]">
        {tabs.map((t) => {
          const selected = t.id === value;
          return (
            <button
              key={t.id}
              id={`${uid}-tab-${t.id}`}
              type="button"
              role="tab"
              aria-selected={selected}
              aria-controls={`${uid}-panel`}
              tabIndex={selected ? 0 : -1}
              onClick={() => onChange(t.id)}
              className={cn(
                'shrink-0 px-3 py-2.5 text-sm font-semibold whitespace-nowrap motion-safe:transition-colors',
                'focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
                selected ? 'text-ink shadow-[inset_0_-2px_0_var(--color-accent)]' : 'text-ink-muted hover:text-ink hover:shadow-[inset_0_-2px_0_var(--color-rule-strong)]',
              )}
            >
              {t.label}
              {t.count !== undefined && <span className="ml-2 rounded-full bg-neutral-tint px-1.5 py-px font-mono text-xs text-ink-muted">{t.count}</span>}
            </button>
          );
        })}
      </div>
      <div role="tabpanel" id={`${uid}-panel`} aria-labelledby={`${uid}-tab-${value}`} className="flex flex-col gap-8">
        {children}
      </div>
    </div>
  );
}
