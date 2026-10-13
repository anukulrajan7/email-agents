import type { ReactNode } from 'react';
import { labelColors } from '../../components/ui';
import { cn } from '../../lib/cn';
import type { AgentBadge } from './types';

export function AgentAvatar({ agent, size = 'sm', className }: { agent: Pick<AgentBadge, 'avatar' | 'color'>; size?: 'sm' | 'lg'; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        'inline-flex shrink-0 items-center justify-center rounded-full border-[1.5px] bg-raised font-semibold leading-none',
        size === 'lg' ? 'h-12 w-12 text-xl' : 'h-6 w-6 text-[11px]',
        labelColors[agent.color]?.ink ?? 'border-rule-strong text-ink-muted',
        className,
      )}
    >
      {agent.avatar}
    </span>
  );
}

/** "Meera (agent)" with the avatar, for cases and approvals. */
export function AgentByline({ agent, children }: { agent: AgentBadge; children?: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-2 text-sm text-ink-muted">
      <AgentAvatar agent={agent} />
      <span>
        <span className="font-semibold text-ink">{agent.name}</span> (agent){children}
      </span>
    </span>
  );
}
