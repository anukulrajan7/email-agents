import { cn } from '../../lib/cn';

export function Switch({
  checked,
  label,
  disabled,
  onChange,
}: {
  checked: boolean;
  label: string;
  disabled?: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors',
        'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50',
        checked ? 'bg-success' : 'bg-ink-muted',
      )}
    >
      <span className={cn('inline-block h-5 w-5 rounded-full bg-white shadow-sm transition-transform', checked ? 'translate-x-5' : 'translate-x-0.5')} />
    </button>
  );
}
