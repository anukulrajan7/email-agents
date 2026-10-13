import { cn } from '../../lib/cn';

export function CountBadge({ count, label, className }: { count: number; label: string; className?: string }) {
  if (count <= 0) return null;
  return (
    <span
      className={cn(
        'ml-auto inline-flex h-5 min-w-5 items-center justify-center rounded-full bg-accent px-1.5 font-mono text-[11px] leading-5 font-semibold text-accent-ink',
        className,
      )}
    >
      <span aria-hidden="true">{count > 99 ? '99+' : count}</span>
      <span className="sr-only">{label}</span>
    </span>
  );
}
