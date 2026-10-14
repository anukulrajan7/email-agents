import type { ReactNode } from 'react';
import { riseStyle } from '../../lib/motion';

export function PageHeader({
  kicker,
  title,
  description,
  actions,
  guide,
}: {
  kicker: string;
  title: string;
  description?: string;
  actions?: ReactNode;
  guide?: string;
}) {
  return (
    <header data-guide={guide} className="rise flex flex-wrap items-end justify-between gap-x-6 gap-y-4 border-b border-rule-strong pb-6" style={riseStyle(0)}>
      <div className="min-w-0 flex-1 basis-80">
        <p className="kicker">{kicker}</p>
        <h1 className="type-display mt-2 text-[2rem] leading-[1.05] tracking-[-0.02em] break-words text-ink sm:text-[2.5rem]">{title}</h1>
        {description && <p className="mt-3 max-w-[62ch] text-[15px] leading-relaxed text-ink-muted">{description}</p>}
      </div>
      {actions && <div className="flex shrink-0 flex-wrap items-center gap-2">{actions}</div>}
    </header>
  );
}
