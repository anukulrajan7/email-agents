import type { ReactNode } from 'react';
import { riseStyle } from '../../lib/motion';

export function PageHeader({
  kicker,
  title,
  description,
  actions,
}: {
  kicker: string;
  title: string;
  description?: string;
  actions?: ReactNode;
}) {
  return (
    <header className="rise flex flex-wrap items-end justify-between gap-4 border-b border-rule-strong pb-6" style={riseStyle(0)}>
      <div className="min-w-0">
        <p className="kicker">{kicker}</p>
        <h1 className="type-display mt-1 text-4xl tracking-[-0.02em] text-ink">{title}</h1>
        {description && <p className="mt-2 text-[15px] text-ink-muted">{description}</p>}
      </div>
      {actions}
    </header>
  );
}
