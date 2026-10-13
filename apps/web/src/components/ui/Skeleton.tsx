import { cn } from '../../lib/cn';

export function Skeleton({ className }: { className?: string }) {
  return <span aria-hidden="true" className={cn('block rounded-xs bg-rule/70 motion-safe:animate-pulse', className)} />;
}

export function SkeletonList({ rows = 3, label = 'Loading', className }: { rows?: number; label?: string; className?: string }) {
  return (
    <div role="status" aria-label={label} aria-busy="true" className={cn('flex flex-col gap-3', className)}>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex flex-col gap-2 rounded-sm border border-rule bg-surface p-4">
          <Skeleton className="h-4 w-2/5" />
          <Skeleton className="h-3 w-4/5" />
          <Skeleton className="h-3 w-3/5" />
        </div>
      ))}
    </div>
  );
}

export function SkeletonTable({ rows = 4, label = 'Loading' }: { rows?: number; label?: string }) {
  return (
    <div role="status" aria-label={label} aria-busy="true" className="flex flex-col divide-y divide-rule rounded-sm border border-rule bg-surface">
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className="flex items-center gap-6 px-4 py-3.5">
          <Skeleton className="h-3 w-1/4" />
          <Skeleton className="h-3 w-1/3" />
          <Skeleton className="ml-auto h-3 w-16" />
        </div>
      ))}
    </div>
  );
}
