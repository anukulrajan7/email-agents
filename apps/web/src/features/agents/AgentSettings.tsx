import { useState } from 'react';
import { Badge, Button, Card, CardTitle, Mono, Skeleton, Table, Td, Th, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { AgentAvatar } from './AgentAvatar';
import { AgentEditor } from './AgentEditor';
import { APPROVAL_OPTIONS, QUALITY_LABEL, SEND_SUMMARY, tasksSummary } from './agentLabels';
import { useAgent, useMyProfile, useSaveMyProfile, useTeamProfiles } from './hooks';
import { ProfileForm } from './ProfileForm';

function CardSkeleton({ label }: { label: string }) {
  return (
    <div role="status" aria-label={label} className="flex flex-col gap-3">
      <Skeleton className="h-12 w-12 rounded-full" />
      <Skeleton className="h-4 w-2/3" />
      <Skeleton className="h-16 w-full" />
    </div>
  );
}

/** "Your agent": the workspace's one lead agent, in plain words. Admins edit it. */
export function LeadAgentCard({ ws }: { ws: string }) {
  const view = useAgent(ws);
  const [editing, setEditing] = useState(false);
  const agent = view.data?.agent;

  return (
    <Card role="region" aria-labelledby="lead-agent-heading" className="flex h-full flex-col gap-4 p-5">
      <CardTitle id="lead-agent-heading">Your agent</CardTitle>
      {view.isLoading && <CardSkeleton label="Loading your agent" />}
      <ErrorNote error={view.error} />
      {agent && view.data && (
        <>
          <div className="flex items-center gap-3">
            <AgentAvatar agent={agent} size="lg" />
            <div className="min-w-0">
              <p className="type-display text-2xl leading-tight tracking-[-0.01em] break-words">{agent.name}</p>
              <p className="text-sm text-ink-muted">{tasksSummary(agent.tasks, agent.enabled)}</p>
            </div>
          </div>
          <p className="line-clamp-4 border-l-[3px] border-rule-strong pl-3 text-sm whitespace-pre-wrap text-ink-muted">
            {agent.persona || 'No instructions yet: the standard guidance is used.'}
          </p>
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
            <dt className="kicker self-center">Drafts</dt>
            <dd>{agent.permissions.draft ? 'Writes reply drafts' : 'Does not write drafts'}</dd>
            <dt className="kicker self-center">Sending</dt>
            <dd>{SEND_SUMMARY[agent.permissions.send]}</dd>
            <dt className="kicker self-center">Quality</dt>
            <dd>{QUALITY_LABEL[agent.quality]}</dd>
          </dl>
          <div className="mt-auto">
            {view.data.can_edit ? (
              <Button variant="secondary" size="sm" onClick={() => setEditing(true)}>
                Edit {agent.name}
              </Button>
            ) : (
              <p className="text-sm text-ink-muted">Only a workspace admin can change the agent.</p>
            )}
          </div>
          {editing && <AgentEditor ws={ws} agent={agent} standardGuidance={view.data.standard_guidance} onClose={() => setEditing(false)} />}
        </>
      )}
    </Card>
  );
}

/** "My profile": what the agent should know about me when it works on my mail. */
export function MyProfileCard({ ws }: { ws: string }) {
  const mine = useMyProfile(ws);
  const save = useSaveMyProfile(ws);
  const notify = useToast();
  const [editing, setEditing] = useState(false);
  const data = mine.data;
  const agentName = data?.agent?.name ?? 'the agent';
  const p = data?.profile;

  return (
    <Card role="region" aria-labelledby="my-profile-heading" className="flex h-full flex-col gap-4 p-5">
      <CardTitle id="my-profile-heading">My profile</CardTitle>
      {mine.isLoading && <CardSkeleton label="Loading your profile" />}
      <ErrorNote error={mine.error} />
      {data && p && !editing && (
        <>
          <p className="text-sm text-ink-muted">
            {p.saved
              ? `${agentName} adapts to you when it works on mail in ${data.mailboxes.length ? data.mailboxes.join(', ') : 'your mailbox'}.`
              : `Tell ${agentName} about you so replies from your mailbox sound like you.`}
          </p>
          {p.saved && (
            <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
              <dt className="kicker self-center">Role</dt>
              <dd className="break-words">{p.job_title || '—'}</dd>
              <dt className="kicker self-center">Tone</dt>
              <dd>{[p.tone.formality, p.tone.length].filter(Boolean).join(', ') || 'No preference'}</dd>
              <dt className="kicker self-center">Sending</dt>
              <dd>{APPROVAL_OPTIONS.find((o) => o.value === p.approvals.send_email)?.label}</dd>
            </dl>
          )}
          <div className="mt-auto">
            <Button variant={p.saved ? 'secondary' : 'primary'} size="sm" onClick={() => setEditing(true)}>
              {p.saved ? 'Edit my profile' : `Tell ${agentName} about you`}
            </Button>
          </div>
        </>
      )}
      {data && p && editing && (
        <ProfileForm
          ws={ws}
          agentName={agentName}
          initial={p}
          saving={save.isPending}
          error={save.error}
          onSave={(input) =>
            save.mutate(input, {
              onSuccess: () => {
                notify('success', 'Profile saved.');
                setEditing(false);
              },
            })
          }
          extraActions={
            <Button variant="secondary" onClick={() => setEditing(false)} disabled={save.isPending}>
              Cancel
            </Button>
          }
        />
      )}
    </Card>
  );
}

/** Admins: everyone's profile at a glance. */
export function TeamProfiles({ ws }: { ws: string }) {
  const team = useTeamProfiles(ws, true);
  const items = team.data?.items ?? [];
  return (
    <section aria-labelledby="team-profiles-heading" className="flex flex-col gap-3">
      <h2 id="team-profiles-heading" className="type-display text-2xl tracking-[-0.01em]">
        Team profiles
      </h2>
      {team.isLoading && <Skeleton className="h-24 w-full" />}
      <ErrorNote error={team.error} />
      {team.data && (
        <Table aria-labelledby="team-profiles-heading">
          <thead>
            <tr>
              <Th>Person</Th>
              <Th>Mailbox</Th>
              <Th>Role</Th>
              <Th>Sending</Th>
            </tr>
          </thead>
          <tbody>
            {items.map((m) => (
              <tr key={m.user_id}>
                <Td>
                  <span className="font-semibold">{m.profile?.display_name || m.email}</span>
                  {m.profile?.display_name && <Mono className="block">{m.email}</Mono>}
                </Td>
                <Td>{m.mailboxes.length ? <Mono>{m.mailboxes.join(', ')}</Mono> : <span className="text-ink-muted">None</span>}</Td>
                <Td>{m.profile ? m.profile.job_title || '—' : <Badge>Not filled in</Badge>}</Td>
                <Td>{m.profile ? APPROVAL_OPTIONS.find((o) => o.value === m.profile?.approvals.send_email)?.label : 'Always ask me'}</Td>
              </tr>
            ))}
          </tbody>
        </Table>
      )}
    </section>
  );
}
