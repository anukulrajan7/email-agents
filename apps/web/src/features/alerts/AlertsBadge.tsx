import { useOpenAlertCount } from './hooks';

export function AlertsBadge({ ws }: { ws: string }) {
  const count = useOpenAlertCount(ws);
  if (count <= 0) return null;
  return (
    <span className="ml-2 inline-flex min-w-5 items-center justify-center rounded-full bg-accent px-1.5 font-mono text-[11px] leading-5 font-semibold text-accent-ink">
      <span aria-hidden="true">{count > 99 ? '99+' : count}</span>
      <span className="sr-only">
        {count} open {count === 1 ? 'alert' : 'alerts'}
      </span>
    </span>
  );
}
