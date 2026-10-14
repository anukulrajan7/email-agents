import { Badge, Button, Switch } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import type { AgentSummary } from '../../api/types';
import { AgentFace } from './AgentFace';
import { resolveAvatar } from './avatars';
import { AUTONOMY, PAUSE_REASON, STATUS_COPY } from './autonomy';

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
  featured = false,
  onToggle,
  onOpen,
}: {
  agent: AgentSummary;
  index: number;
  isAdmin: boolean;
  busy?: boolean;
  featured?: boolean;
  onToggle: (agent: AgentSummary, on: boolean) => void;
  onOpen: (agent: AgentSummary) => void;
}) {
  const status = STATUS_COPY[agent.status];
  const active = agent.status === 'active';
  const toggle = canToggle(agent, isAdmin);
  const waiting = agent.today.approvals_pending;
  const why = agent.status === 'paused' && agent.paused_reason ? PAUSE_REASON[agent.paused_reason] : undefined;

  return (
    <article
      aria-label={agent.name}
      className="group rise letter flex h-full flex-col gap-4 rounded-xs p-5"
      style={riseStyle(index + 2)}
      data-agent-id={agent.id}
    >
      <div className="flex items-start gap-4">
        <AgentFace avatar={resolveAvatar(agent)} size={featured ? 'xl' : 'lg'} index={index} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h3 className="type-display text-xl leading-tight tracking-[-0.01em] break-words">{agent.name}</h3>
            {agent.kind === 'lead' && <Badge tone="info">Lead</Badge>}
          </div>
          <p className="mt-1 text-sm leading-snug text-ink-muted">{agent.description}</p>
        </div>
      </div>

      <div className="flex flex-col gap-1.5 text-sm">
        <p className="flex flex-wrap items-center gap-2">
          <Badge tone={status.tone}>{status.label}</Badge>
          <Badge>{AUTONOMY[agent.autonomy.effective].chip}</Badge>
          {agent.update_available && <Badge tone="warning">Update available</Badge>}
        </p>
        <p className="text-ink">{AUTONOMY[agent.autonomy.effective].sentence}</p>
        {why && <p className="text-ink-muted">{why}</p>}
      </div>

      <p className="font-mono text-xs text-ink-muted">
        Today: {agent.today.cases} handled
        {waiting > 0 && <span className="text-accent"> · {waiting} waiting on you</span>}
      </p>

      <div className="mt-auto flex items-center justify-between gap-3 border-t border-rule pt-3">
        <span className="flex items-center gap-2 text-[13px] font-semibold text-ink-muted">
          <Switch
            checked={active}
            label={active ? `Pause ${agent.name}` : `Turn on ${agent.name}`}
            disabled={!toggle || busy}
            onChange={(on) => onToggle(agent, on)}
          />
          <span aria-hidden="true">{active ? 'On' : 'Off'}</span>
        </span>
        <Button variant="secondary" size="sm" aria-label={`Open ${agent.name}`} onClick={() => onOpen(agent)}>
          Tune and history
        </Button>
      </div>
    </article>
  );
}
