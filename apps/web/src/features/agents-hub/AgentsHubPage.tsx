import { useState } from 'react';
import { Link } from 'react-router';
import { Button, Page, PageHeader, Skeleton, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import type { AgentCategory, AgentSummary } from '../../api/types';
import { HelpPopover } from '../help/HelpPopover';
import { AgentCard } from './AgentCard';
import { agentDisplayName } from './agentCopy';
import { AgentDrawer } from './AgentDrawer';
import { CATEGORY_ORDER, CATEGORY_TITLE } from './autonomy';
import { FriendlyEmpty } from './FriendlyEmpty';
import { useAgentRegistry, usePatchAgent, usePauseAll } from './hooks';

export type HubGroup = { key: AgentCategory; title: string; agents: AgentSummary[] };

/** Groups by category in a fixed order, drops empty groups, and lifts the user's own agent out into its own spot. */
export function groupAgents(categories: { key: AgentCategory; agents: AgentSummary[] }[], mineId: string | undefined): HubGroup[] {
  const byKey = new Map<AgentCategory, AgentSummary[]>();
  for (const c of categories) {
    const rest = c.agents.filter((a) => a.id !== mineId && a.status !== 'archived');
    byKey.set(c.key, [...(byKey.get(c.key) ?? []), ...rest]);
  }
  return CATEGORY_ORDER.filter((key) => (byKey.get(key)?.length ?? 0) > 0).map((key) => ({
    key,
    title: CATEGORY_TITLE[key],
    agents: [...(byKey.get(key) ?? [])].sort((a, b) => Number(b.kind === 'lead') - Number(a.kind === 'lead') || a.name.localeCompare(b.name)),
  }));
}

export function AgentsHubPage() {
  const { ws, isAdmin } = useWorkspace();
  const registry = useAgentRegistry(ws);
  const patch = usePatchAgent(ws);
  const pauseAll = usePauseAll(ws);
  const notify = useToast();
  const [openId, setOpenId] = useState<string>();

  const data = registry.data;
  const groups = data ? groupAgents(data.categories, data.me?.id) : [];
  const all = data ? [...(data.me ? [data.me] : []), ...data.categories.flatMap((c) => c.agents)] : [];
  const lead = all.find((a) => a.kind === 'lead');
  const open = all.find((a) => a.id === openId);
  const empty = data !== undefined && !data.me && groups.length === 0;
  let index = 0;

  const toggle = (agent: AgentSummary, on: boolean) =>
    patch.mutate(
      { id: agent.id, status: on ? 'active' : 'paused' },
      { onSuccess: () => notify('success', on ? `${agentDisplayName(agent)} is back to work` : `${agent.name} is paused`) },
    );

  return (
    <Page>
      <PageHeader
        kicker="Your team"
        title="Agents"
        description="Each agent looks after one kind of email. They start careful and ask before sending, and you decide how much freedom they get."
        actions={
          <>
            <HelpPopover topic="agents" />
            <HelpPopover topic="autonomy" label="About freedom levels" />
            {isAdmin && (
              <Link
                to={`/w/${ws}/agents/new`}
                className="inline-flex h-8 items-center rounded-sm bg-accent px-3 text-[13px] font-semibold text-accent-ink hover:bg-accent-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
              >
                Create an agent
              </Link>
            )}
            {isAdmin && data && (
              <Button
                variant="secondary"
                size="sm"
                loading={pauseAll.isPending}
                onClick={() => pauseAll.mutate(!data.paused_all, { onSuccess: () => notify('success', data.paused_all ? 'Everyone is back to work' : 'All agents paused') })}
              >
                {data.paused_all ? 'Resume all' : 'Pause all'}
              </Button>
            )}
          </>
        }
      />
      {data && (
        <aside aria-label="About your team" className="rise letter flex flex-col gap-1 rounded-xs p-4 text-sm leading-snug" style={riseStyle(1)}>
          <p className="type-display text-lg tracking-[-0.01em]">Your team</p>
          <p className="text-ink">
            Meera is your own assistant. {lead?.name ?? 'Aarav'} leads the team and hands each email to the right helper. Each helper handles one kind of email.
            New helpers start by only watching.
          </p>
        </aside>
      )}
      {registry.isLoading && <Skeleton className="h-48" />}
      <ErrorNote error={registry.error ?? patch.error ?? pauseAll.error} />
      {data?.paused_all && (
        <p role="status" className="rounded-sm border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
          All agents are paused. Nothing is being sorted or sent until you resume.
        </p>
      )}
      {empty && (
        <FriendlyEmpty
          title="No agents yet"
          text="Agents arrive with your packs. Turn a pack on and its team shows up here."
          action={
            isAdmin ? (
              <Link to={`/w/${ws}/packs`} className="text-sm font-semibold text-accent underline underline-offset-[3px]">
                Choose packs
              </Link>
            ) : undefined
          }
        />
      )}
      {data?.me && (
        <section aria-labelledby="hub-mine" className="flex flex-col gap-4">
          <div className="rise" style={riseStyle(1)}>
            <h2 id="hub-mine" className="type-display text-2xl tracking-[-0.01em]">
              Your personal agent
            </h2>
            <p className="text-sm text-ink-muted">Works only on your own mailbox. Nobody else can see it, admins included.</p>
          </div>
          <div className="grid max-w-xl grid-cols-1">
            <AgentCard agent={data.me} index={index++} featured isAdmin={isAdmin} busy={patch.isPending} onToggle={toggle} onOpen={(a) => setOpenId(a.id)} />
          </div>
        </section>
      )}
      {groups.map((g) => (
        <section key={g.key} aria-labelledby={`hub-${g.key}`} className="flex flex-col gap-4">
          <h2 id={`hub-${g.key}`} className="type-display flex items-baseline gap-3 text-2xl tracking-[-0.01em]">
            {g.title}
            <span className="font-mono text-xs text-ink-muted">{g.agents.length}</span>
          </h2>
          <ul className="grid auto-rows-fr grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
            {g.agents.map((a) => (
              <li key={a.id}>
                <AgentCard agent={a} index={index++} isAdmin={isAdmin} busy={patch.isPending} onToggle={toggle} onOpen={(x) => setOpenId(x.id)} />
              </li>
            ))}
          </ul>
        </section>
      ))}
      {open && <AgentDrawer ws={ws} agent={open} isAdmin={isAdmin} onClose={() => setOpenId(undefined)} />}
    </Page>
  );
}
