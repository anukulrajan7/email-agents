import type { ReactNode } from 'react';

export function EmptyState({ title, description, action }: { title: string; description?: string; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-3 px-6 py-16 text-center">
      <svg aria-hidden="true" viewBox="0 0 48 32" className="h-8 w-12 text-rule-strong" fill="none" stroke="currentColor" strokeWidth="1.5">
        <rect x="1.5" y="1.5" width="45" height="29" rx="2" />
        <path d="M2 3l22 17L46 3" />
        <path d="M2 29l16-14M46 29L30 15" strokeDasharray="2 3" />
      </svg>
      <p className="type-display text-2xl tracking-[-0.01em] text-ink">{title}</p>
      {description && <p className="max-w-sm text-sm text-ink-muted">{description}</p>}
      {action}
    </div>
  );
}
