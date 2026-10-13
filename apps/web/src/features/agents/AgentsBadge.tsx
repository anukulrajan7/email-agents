import { usePendingApprovalCount } from './hooks';

export function AgentsBadge({ ws }: { ws: string }) {
  const count = usePendingApprovalCount(ws);
  if (count <= 0) return null;
  return (
    <span className="ml-2 inline-flex min-w-5 items-center justify-center rounded-full bg-accent px-1.5 font-mono text-[11px] leading-5 font-semibold text-accent-ink">
      <span aria-hidden="true">{count > 99 ? '99+' : count}</span>
      <span className="sr-only">
        {count} {count === 1 ? 'approval' : 'approvals'} waiting
      </span>
    </span>
  );
}
