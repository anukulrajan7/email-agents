import type { ButtonHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';
import { Spinner } from './Spinner';

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger';

const variants: Record<Variant, string> = {
  primary: 'bg-accent text-accent-ink hover:bg-accent-hover disabled:bg-rule disabled:text-ink-muted',
  secondary:
    'border border-rule-strong bg-raised text-ink hover:border-ink hover:bg-surface disabled:border-rule disabled:bg-transparent disabled:text-ink-muted',
  ghost: 'text-ink hover:bg-ink/5 disabled:text-ink-muted',
  danger:
    'border border-danger bg-raised text-danger hover:bg-danger hover:text-accent-ink disabled:border-rule disabled:bg-transparent disabled:text-ink-muted',
};

type Props = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: 'sm' | 'md';
  loading?: boolean;
};

export function Button({ variant = 'primary', size = 'md', loading, className, disabled, children, ...rest }: Props) {
  return (
    <button
      type="button"
      {...rest}
      disabled={disabled || loading}
      className={cn(
        'inline-flex items-center justify-center gap-2 rounded-sm font-semibold whitespace-nowrap transition-colors',
        'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
        'motion-safe:active:translate-y-px disabled:cursor-not-allowed',
        size === 'sm' ? 'h-8 px-3 text-[13px]' : 'h-10 px-4 text-sm',
        variants[variant],
        className,
      )}
    >
      {loading && <Spinner className="h-4 w-4" />}
      {children}
    </button>
  );
}
