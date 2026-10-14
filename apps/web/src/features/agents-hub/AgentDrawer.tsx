import { useState } from 'react';
import { Badge, Button, Dialog, Input, Label, Select, Skeleton, Table, Tabs, Td, Th, RelativeTime, useToast, type TabItem } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import type { AgentSummary, Autonomy } from '../../api/types';
import { AgentFace } from './AgentFace';
import { AvatarPicker } from './AvatarPicker';
import { resolveAvatar } from './avatars';
import { AUTONOMY, LIMITED_BY } from './autonomy';
import { FriendlyEmpty } from './FriendlyEmpty';
import { useActivateVersion, useAgentAudit, useAgentOutgoing, useAgentVersions, usePatchAgent, useStopSend } from './hooks';

type Tab = 'tune' | 'versions' | 'outgoing' | 'audit';

const TABS: TabItem<Tab>[] = [
  { id: 'tune', label: 'Tune' },
  { id: 'versions', label: 'Versions' },
  { id: 'outgoing', label: 'Going out' },
  { id: 'audit', label: 'Activity log' },
];

function TuneTab({ ws, agent, canEdit }: { ws: string; agent: AgentSummary; canEdit: boolean }) {
  const patch = usePatchAgent(ws);
  const notify = useToast();
  const [name, setName] = useState(agent.name);
  const [cap, setCap] = useState<Autonomy>(agent.autonomy.cap);
  const avatar = resolveAvatar(agent);
  const dirty = name.trim() !== agent.name || cap !== agent.autonomy.cap;
  const personal = agent.scope === 'personal';

  const save = () =>
    patch.mutate(
      { id: agent.id, name: name.trim(), autonomy_cap: cap },
      { onSuccess: () => notify('success', 'Saved') },
    );

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-2">
        <p className="kicker">Avatar</p>
        <AvatarPicker
          label="Choose an avatar"
          value={avatar}
          disabled={!canEdit || patch.isPending}
          onChange={(stored) => patch.mutate({ id: agent.id, avatar: stored })}
        />
        {!canEdit && <p className="text-sm text-ink-muted">Only {personal ? 'the owner' : 'an admin'} can change this agent.</p>}
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor="agent-name">Name</Label>
        <Input id="agent-name" value={name} maxLength={40} disabled={!canEdit} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor="agent-cap">How much freedom</Label>
        <Select id="agent-cap" value={cap} disabled={!canEdit} onChange={(e) => setCap(Number(e.target.value) as Autonomy)}>
          {([0, 1, 2, 3] as Autonomy[]).map((level) => (
            <option key={level} value={level}>
              {AUTONOMY[level].sentence}
            </option>
          ))}
        </Select>
        <p className="text-sm text-ink-muted">
          Right now: {AUTONOMY[agent.autonomy.effective].sentence}.
          {agent.autonomy.limited_by !== 'earned' || agent.autonomy.effective < agent.autonomy.cap
            ? ` ${LIMITED_BY[agent.autonomy.limited_by] ?? ''}.`
            : ''}
        </p>
      </div>
      <ErrorNote error={patch.error} />
      <div>
        <Button disabled={!canEdit || !dirty || name.trim() === ''} loading={patch.isPending} onClick={save}>
          Save changes
        </Button>
      </div>
    </div>
  );
}

function VersionsTab({ ws, agent, canEdit }: { ws: string; agent: AgentSummary; canEdit: boolean }) {
  const versions = useAgentVersions(ws, agent.id);
  const activate = useActivateVersion(ws, agent.id);
  const notify = useToast();
  const items = versions.data?.items ?? [];
  return (
    <div className="flex flex-col gap-3">
      {agent.update_available && <p className="rounded-sm border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">A newer version of this agent is available from its pack.</p>}
      {versions.isLoading && <Skeleton className="h-24" />}
      <ErrorNote error={versions.error ?? activate.error} />
      {versions.data && items.length === 0 && <FriendlyEmpty title="No saved versions yet" text="Each time the setup changes, a version is saved here so you can go back." />}
      <ul className="flex flex-col divide-y divide-rule rounded-sm border border-rule bg-surface">
        {items.map((v) => (
          <li key={v.version} className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 text-sm">
            <div className="min-w-0">
              <p className="font-semibold">
                Version {v.version} {v.active && <Badge tone="success">Current</Badge>}
              </p>
              <p className="text-ink-muted">
                {v.note ?? SOURCE_COPY[v.source]} · <RelativeTime iso={v.created_at} />
              </p>
            </div>
            {!v.active && (
              <Button
                variant="secondary"
                size="sm"
                disabled={!canEdit}
                loading={activate.isPending && activate.variables === v.version}
                aria-label={`Roll back to version ${v.version}`}
                onClick={() => activate.mutate(v.version, { onSuccess: () => notify('success', `Back on version ${v.version}`) })}
              >
                Roll back to this
              </Button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

const SOURCE_COPY = {
  template: 'Standard setup',
  tuned: 'Tuned by your team',
  designer: 'Built with the designer',
  manual: 'Edited by hand',
  rollback: 'Rolled back',
  backfill: 'Imported',
} as const;

function OutgoingTab({ ws, agent, canStop }: { ws: string; agent: AgentSummary; canStop: boolean }) {
  const outgoing = useAgentOutgoing(ws, agent.id);
  const stop = useStopSend(ws, agent.id);
  const notify = useToast();
  const items = outgoing.data?.items ?? [];
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-ink-muted">Replies {agent.name} sends on its own wait here for a short hold. Press Stop to cancel one before it leaves.</p>
      {outgoing.isLoading && <Skeleton className="h-20" />}
      <ErrorNote error={outgoing.error ?? stop.error} />
      {outgoing.data && items.length === 0 && <FriendlyEmpty title="Nothing waiting to go out" text="When this agent is about to send a reply, you will see it here with a Stop button." />}
      <ul className="flex flex-col gap-3">
        {items.map((o) => (
          <li key={o.draft_id} className="letter flex flex-col gap-2 rounded-xs p-4 text-sm">
            <div className="flex flex-wrap items-start justify-between gap-3">
              <div className="min-w-0">
                <p className="font-semibold break-words">{o.subject}</p>
                <p className="text-ink-muted">
                  To {o.to} · <RelativeTime iso={o.send_at} prefix="sends " />
                </p>
              </div>
              <Button
                variant="danger"
                size="sm"
                disabled={!canStop}
                loading={stop.isPending && stop.variables === o.draft_id}
                aria-label={`Stop sending ${o.subject}`}
                onClick={() => stop.mutate(o.draft_id, { onSuccess: () => notify('success', 'Stopped. It will not be sent.') })}
              >
                Stop
              </Button>
            </div>
            <p className="line-clamp-3 text-ink-muted">{o.body_preview}</p>
          </li>
        ))}
      </ul>
    </div>
  );
}

const AUDIT_STATUS = { ok: 'Done', error: 'Failed', blocked: 'Blocked', waiting_approval: 'Waiting for you', simulated: 'Practice run' } as const;

function AuditTab({ ws, agent }: { ws: string; agent: AgentSummary }) {
  const audit = useAgentAudit(ws, agent.id);
  const items = audit.data?.items ?? [];
  return (
    <div className="flex flex-col gap-3">
      {audit.isLoading && <Skeleton className="h-24" />}
      <ErrorNote error={audit.error} />
      {audit.data && items.length === 0 && <FriendlyEmpty title="No activity yet" text="Every step this agent takes is recorded here, so you can always see what it did." />}
      {items.length > 0 && (
        <Table>
          <thead>
            <tr>
              <Th>When</Th>
              <Th>Action</Th>
              <Th>Result</Th>
            </tr>
          </thead>
          <tbody>
            {items.map((a, i) => (
              <tr key={`${a.case_id}-${a.turn}-${i}`}>
                <Td>
                  <RelativeTime iso={a.at} />
                </Td>
                <Td className="font-mono text-xs">{a.tool}</Td>
                <Td>
                  {AUDIT_STATUS[a.status]}
                  {a.shadow && <span className="text-ink-muted"> (watching)</span>}
                </Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </div>
  );
}

export function AgentDrawer({ ws, agent, isAdmin, onClose }: { ws: string; agent: AgentSummary; isAdmin: boolean; onClose: () => void }) {
  const [tab, setTab] = useState<Tab>('tune');
  const personal = agent.scope === 'personal';
  const canEdit = personal || isAdmin;
  return (
    <Dialog title={agent.name} variant="drawer" onClose={onClose}>
      <div className="flex items-center gap-3">
        <AgentFace avatar={resolveAvatar(agent)} size="lg" />
        <p className="text-sm text-ink-muted">{agent.description}</p>
      </div>
      <Tabs label={`${agent.name} settings`} tabs={TABS} value={tab} onChange={setTab}>
        {tab === 'tune' && <TuneTab key={agent.id} ws={ws} agent={agent} canEdit={canEdit} />}
        {tab === 'versions' && <VersionsTab ws={ws} agent={agent} canEdit={canEdit} />}
        {tab === 'outgoing' && <OutgoingTab ws={ws} agent={agent} canStop />}
        {tab === 'audit' && <AuditTab ws={ws} agent={agent} />}
      </Tabs>
    </Dialog>
  );
}
