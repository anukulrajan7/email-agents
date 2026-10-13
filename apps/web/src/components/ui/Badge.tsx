import type { ReactNode } from 'react';
import { cn } from '../../lib/cn';
import type { Category } from '../../api/types';
import { humanize } from '../../lib/format';

export type Tone = 'neutral' | 'info' | 'success' | 'warning' | 'danger';

const tones: Record<Tone, string> = {
  neutral: 'border-rule-strong bg-neutral-tint text-ink-muted',
  info: 'border-info/40 bg-info-tint text-info',
  success: 'border-success/40 bg-success-tint text-success',
  warning: 'border-review/50 bg-review-tint text-review-ink',
  danger: 'border-danger/40 bg-danger-tint text-danger',
};

export function Badge({ tone = 'neutral', children, className }: { tone?: Tone; children: ReactNode; className?: string }) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded-sm border px-1.5 py-px text-[11px] leading-4 font-semibold tracking-[0.08em] whitespace-nowrap uppercase',
        tones[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

const inks: Record<Category, string> = {
  timesheet: 'border-info text-info',
  leave: 'border-success text-success',
  candidate_referral: 'border-plum text-plum',
  approval: 'border-review text-review-ink',
  payroll: 'border-payroll text-payroll',
  general: 'border-ink-muted text-ink-muted',
  other: 'border-rule-strong text-ink-muted',
};

export function Stamp({
  category,
  tilted = false,
  size = 'sm',
  className,
}: {
  category: Category;
  tilted?: boolean;
  size?: 'sm' | 'lg';
  className?: string;
}) {
  return (
    <span
      className={cn(
        'inline-flex items-center rounded-xs border-[1.5px] font-mono font-semibold tracking-[0.14em] whitespace-nowrap uppercase',
        size === 'lg' ? 'px-3 py-1 text-[13px]' : 'px-1.5 py-px text-[10px] leading-4',
        tilted && '-rotate-[1.5deg]',
        inks[category],
        className,
      )}
    >
      {humanize(category)}
    </span>
  );
}
