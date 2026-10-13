import type { HTMLAttributes, TableHTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export function Table({ className, ...rest }: TableHTMLAttributes<HTMLTableElement>) {
  return (
    <div className="overflow-x-auto rounded-sm border border-rule bg-surface">
      <table {...rest} className={cn('ledger w-full text-left text-sm', className)} />
    </div>
  );
}

export function Th({ className, ...rest }: ThHTMLAttributes<HTMLTableCellElement>) {
  return <th scope="col" {...rest} className={className} />;
}

export function Td({ className, ...rest }: TdHTMLAttributes<HTMLTableCellElement>) {
  return <td {...rest} className={className} />;
}

export function Mono({ className, ...rest }: HTMLAttributes<HTMLSpanElement>) {
  return <span {...rest} className={cn('font-mono text-xs text-ink-muted', className)} />;
}
