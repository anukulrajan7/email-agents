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
    const step = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (step === 0) return;
    e.preventDefault();
    const next = tabs[(index + step + tabs.length) % tabs.length];
    if (!next) return;
    onChange(next.id);
    document.getElementById(`${uid}-tab-${next.id}`)?.focus();
  };

  return (
    <div className="flex flex-col gap-5">
      <div role="tablist" aria-label={label} onKeyDown={onKeyDown} className="flex flex-wrap gap-1 border-b border-rule">
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
                '-mb-px border-b-2 px-3 py-2 text-sm font-semibold motion-safe:transition-colors',
                'focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-accent',
                selected ? 'border-accent text-ink' : 'border-transparent text-ink-muted hover:text-ink',
              )}
            >
              {t.label}
              {t.count !== undefined && <span className="ml-2 font-mono text-xs text-ink-muted">{t.count}</span>}
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
