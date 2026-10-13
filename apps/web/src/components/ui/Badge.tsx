import type { ReactNode } from 'react';
import { cn } from '../../lib/cn';

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

export const labelColors: Record<string, { ink: string; swatch: string }> = {
  slate: { ink: 'border-info text-info', swatch: 'bg-info' },
  sage: { ink: 'border-success text-success', swatch: 'bg-success' },
  plum: { ink: 'border-plum text-plum', swatch: 'bg-plum' },
  ochre: { ink: 'border-review text-review-ink', swatch: 'bg-review' },
  teal: { ink: 'border-payroll text-payroll', swatch: 'bg-payroll' },
  postmark: { ink: 'border-accent text-accent', swatch: 'bg-accent' },
  ink: { ink: 'border-ink text-ink', swatch: 'bg-ink' },
  rule: { ink: 'border-rule-strong text-ink-muted', swatch: 'bg-rule-strong' },
};

const mutedInk = 'border-rule-strong text-ink-muted';

export function Stamp({
  name,
  color,
  tilted = false,
  size = 'sm',
  className,
}: {
  name: string;
  color?: string;
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
        (color && labelColors[color]?.ink) || mutedInk,
        className,
      )}
    >
      {name}
    </span>
  );
}
