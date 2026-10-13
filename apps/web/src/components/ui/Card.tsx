import type { HTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export function Card({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div {...rest} className={cn('rounded-sm border border-rule bg-surface p-4 shadow-[0_1px_0_var(--color-rule)]', className)} />;
}

export function CardTitle({ className, ...rest }: HTMLAttributes<HTMLHeadingElement>) {
  return <h3 {...rest} className={cn('type-display text-xl tracking-[-0.01em] text-ink', className)} />;
}
