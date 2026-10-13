import { cn } from '../../lib/cn';

export function SegmentedControl<T extends string | number>({
  label,
  options,
  value,
  onChange,
  className,
}: {
  label: string;
  options: { value: T; label: string }[];
  value: T;
  onChange: (value: T) => void;
  className?: string;
}) {
  return (
    <div
      role="group"
      aria-label={label}
      className={cn('inline-flex self-start divide-x divide-rule-strong overflow-hidden rounded-sm border border-rule-strong bg-raised', className)}
    >
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button
            key={String(o.value)}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(o.value)}
            className={cn(
              'h-8 px-3 text-[13px] font-semibold whitespace-nowrap motion-safe:transition-colors',
              'focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
              active ? 'bg-ink text-accent-ink' : 'text-ink-muted hover:bg-ink/5 hover:text-ink',
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
