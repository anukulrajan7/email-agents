import { useId, useState, type FormEvent } from 'react';
import { Button, Card, CardTitle, EmptyState, Input, Label, Page, PageHeader, SkeletonList, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { EmailChipInput } from '../alerts/EmailChipInput';
import { useEscalations, useSaveTeamSettings, useTeamAccess, useTeamSettings } from './hooks';
import type { TeamSettings } from './types';

function DelaysForm({ initial, current }: { initial: TeamSettings; current: { manager: number; final: number } }) {
  const { ws } = useWorkspace();
  const uid = useId();
  const save = useSaveTeamSettings(ws);
  const notify = useToast();
  const [manager, setManager] = useState(initial.manager_after_hours ?? current.manager);
  const [final, setFinal] = useState(initial.final_after_hours ?? current.final);
  const [emails, setEmails] = useState(initial.final_emails);
  const [byOwner, setByOwner] = useState(initial.route_alerts_by_owner ?? false);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate({ ...initial, manager_after_hours: manager, final_after_hours: final, final_emails: emails, route_alerts_by_owner: byOwner }, { onSuccess: () => notify('success', 'Escalation settings saved.') });
  };

  return (
    <Card>
      <form onSubmit={submit} className="flex flex-col gap-4">
        <div className="flex flex-wrap gap-6">
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-m`}>Hours with the owner before the manager is told</Label>
            <Input id={`${uid}-m`} type="number" min={1} max={720} className="w-28" value={manager} onChange={(e) => setManager(Number(e.target.value))} />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-f`}>Hours with the manager before the final contacts are told</Label>
            <Input id={`${uid}-f`} type="number" min={1} max={720} className="w-28" value={final} onChange={(e) => setFinal(Number(e.target.value))} />
          </div>
        </div>
        <EmailChipInput label="Final contacts (HR or admins)" hint="Leave empty to use the alert recipients." value={emails} onChange={setEmails} />
        <label className="flex items-center gap-2 text-sm text-ink">
          <input type="checkbox" checked={byOwner} onChange={(e) => setByOwner(e.target.checked)} />
          Tell the owner of the mailbox first (always on for HR &amp; recruiting)
        </label>
        <ErrorNote error={save.error} />
        <div>
          <Button type="submit" loading={save.isPending}>
            Save
          </Button>
        </div>
      </form>
    </Card>
  );
}

export function EscalationsPage() {
  const { ws } = useWorkspace();
  const esc = useEscalations(ws);
  const access = useTeamAccess(ws);
  const settings = useTeamSettings(ws);
  const staffing = access.data?.staffing ?? false;
  const chains = esc.data?.chains ?? [];

  return (
    <Page className="gap-6">
      <PageHeader
        kicker={staffing ? 'HRMS · Escalations' : 'Team · Escalations'}
        title="Escalations"
        description="When an alert is not dealt with, it moves from the person it is about, to their manager, to HR (or the admins), after the delays below."
      />
      <ErrorNote error={esc.error ?? settings.error} />
      {(esc.isLoading || settings.isLoading) && <SkeletonList rows={3} label="Loading escalations" />}
      {esc.data?.can_edit && settings.data && (
        <DelaysForm key={JSON.stringify(settings.data)} initial={settings.data} current={{ manager: esc.data.manager_after_hours, final: esc.data.final_after_hours }} />
      )}
      {esc.data && !esc.data.can_edit && (
        <p className="text-sm text-ink-muted">
          Alerts wait {esc.data.manager_after_hours} h with the owner and {esc.data.final_after_hours} h with the manager. Only the super admin changes this.
        </p>
      )}
      {esc.data &&
        (chains.length === 0 ? (
          <Card>
            <EmptyState
              title="No chains to show yet"
              description="Escalation follows the manager chain in People. Add people with their manager, and each person's chain appears here."
            />
          </Card>
        ) : (
          <section aria-label="Escalation chains" className="rise flex flex-col gap-3" style={riseStyle(1)}>
            <CardTitle>Who is told, in order</CardTitle>
            <ul className="flex flex-col gap-2">
              {chains.map((c) => (
                <li key={c.person.id} className="rounded-sm border border-rule bg-surface px-3 py-2">
                  <p className="font-semibold text-ink">{c.person.name}</p>
                  <ol className="mt-1 flex flex-wrap gap-x-6 gap-y-1 text-sm text-ink-muted">
                    {c.steps.map((s) => (
                      <li key={s.level}>
                        {s.level}. {s.label}: <span className="text-ink">{s.emails.join(', ') || s.name || 'set by the super admin'}</span>
                      </li>
                    ))}
                  </ol>
                </li>
              ))}
            </ul>
          </section>
        ))}
    </Page>
  );
}
