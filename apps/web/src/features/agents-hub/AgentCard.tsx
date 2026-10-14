import { Badge, Button, Switch } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import type { AgentSummary } from '../../api/types';
import { AgentFace } from './AgentFace';
import { resolveAvatar } from './avatars';
import { PAUSE_REASON, STATUS_COPY } from './autonomy';
import { AUTONOMY_PLAIN, agentCopy, agentDisplayName } from './agentCopy';
import { AgentExplainer } from './AgentExplainer';

export function canToggle(agent: AgentSummary, isAdmin: boolean): boolean {
  if (agent.kind === 'lead' || agent.status === 'archived') return false;
  if (agent.paused_reason === 'pack_off') return false;
  return agent.scope === 'personal' ? true : isAdmin;
}

export function AgentCard({
  agent,
  index,
  isAdmin,
  busy,
  onToggle,
  onOpen,
}: {
  agent: AgentSummary;
  index: number;
  isAdmin: boolean;
  busy?: boolean;
  onToggle: (agent: AgentSummary, on: boolean) => void;
  onOpen: (agent: AgentSummary) => void;
}) {
  const status = STATUS_COPY[agent.status];
  const active = agent.status === 'active';
  const toggle = canToggle(agent, isAdmin);
  const waiting = agent.today.approvals_pending;
  const name = agentDisplayName(agent);
  const why = agent.status === 'paused' && agent.paused_reason ? PAUSE_REASON[agent.paused_reason] : undefined;

  return (
    <article aria-label={name} className="group rise letter flex h-full flex-col gap-2 rounded-xs p-4" style={riseStyle(index + 2)} data-agent-id={agent.id}>
      <div className="flex items-center gap-3">
        <AgentFace avatar={resolveAvatar(agent)} size="card" index={index} />
        <div className="flex min-w-0 flex-1 flex-wrap items-baseline gap-x-2">
          <h3 className="type-display text-lg leading-tight tracking-[-0.01em] break-words">{name}</h3>
          <p className="text-sm leading-snug text-ink-muted">{agentCopy(agent).role}</p>
        </div>
      </div>

      <p className="flex flex-wrap items-center gap-2 text-sm">
        <Badge tone={status.tone}>{status.label}</Badge>
        <span className="font-semibold text-ink">{AUTONOMY_PLAIN[agent.autonomy.effective].headline}</span>
        {agent.update_available && <Badge tone="warning">Update available</Badge>}
      </p>
      {why && <p className="text-sm text-ink-muted">{why}</p>}

      <AgentExplainer agent={agent} />

      <p className="font-mono text-xs text-ink-muted">
        Today: {agent.today.cases} handled
        {waiting > 0 && <span className="text-accent"> · {waiting} waiting on you</span>}
      </p>

      <div className="mt-auto flex items-center justify-between gap-3 border-t border-rule pt-2">
        <span className="flex items-center gap-2 text-[13px] font-semibold text-ink-muted">
          <Switch
            checked={active}
            label={active ? `Pause ${name}` : `Turn on ${name}`}
            disabled={!toggle || busy}
            onChange={(on) => onToggle(agent, on)}
          />
          <span aria-hidden="true">{active ? 'On' : 'Off'}</span>
        </span>
        <Button variant="secondary" size="sm" aria-label={`Open ${name}`} onClick={() => onOpen(agent)}>
          Tune and history
        </Button>
      </div>
    </article>
  );
}
