import { useId, useState, type FormEvent } from 'react';
import { Link } from 'react-router';
import { Badge, Button, Card, EmptyState, Input, Label, Page, PageHeader, Select, SkeletonList, Table, Td, Th, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { useSaveTeamSettings, useTeamAccess, useTeamSettings } from '../team/hooks';
import type { TeamSettings } from '../team/types';
import { useTimesheets } from './hooks';

const TONE = { received: 'success', pending: 'neutral', missing: 'danger' } as const;
const LABEL = { received: 'Received', pending: 'Not due yet', missing: 'Missing' } as const;

function PolicyForm({ ws, settings }: { ws: string; settings: TeamSettings }) {
  const uid = useId();
  const save = useSaveTeamSettings(ws);
  const notify = useToast();
  const [hr, setHr] = useState(settings.hr_source_id ?? '');
  const [grace, setGrace] = useState(settings.timesheet_grace_days ?? 3);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate({ ...settings, hr_source_id: hr || null, timesheet_grace_days: grace }, { onSuccess: () => notify('success', 'Billable policy saved.') });
  };

  return (
    <Card>
      <form onSubmit={submit} className="flex flex-col gap-4">
        <p className="text-sm text-ink-muted">
          Billable people's mail is read only when it arrives through the company HR mailbox. Pick that mailbox here; mail to or from a billable person in any other mailbox is not stored.
        </p>
        <div className="flex flex-wrap gap-6">
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-hr`}>HR / timesheet mailbox</Label>
            <Select id={`${uid}-hr`} value={hr} onChange={(e) => setHr(e.target.value)}>
              <option value="">Not set (policy off)</option>
              {(settings.sources ?? []).map((s) => (
                <option key={s.id} value={s.id}>
                  {s.mailbox ?? s.kind}
                </option>
              ))}
            </Select>
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-grace`}>Days after month end before a missing timesheet is flagged</Label>
            <Input id={`${uid}-grace`} type="number" min={0} max={28} className="w-24" value={grace} onChange={(e) => setGrace(Number(e.target.value))} />
          </div>
        </div>
        <ErrorNote error={save.error} />
        <div>
          <Button type="submit" loading={save.isPending}>
            Save policy
          </Button>
        </div>
      </form>
    </Card>
  );
}

export function TimesheetsPage() {
  const { ws } = useWorkspace();
  const access = useTeamAccess(ws);
  const settings = useTeamSettings(ws);
  const sheets = useTimesheets(ws);
  const items = sheets.data?.items ?? [];
  const owner = access.data?.level === 'super_admin';

  return (
    <Page className="gap-6">
      <PageHeader
        kicker="HRMS · Timesheets"
        title="Timesheets"
        description="Billable people send a timesheet each month. If one has not arrived three days after month end, the person is told and then their manager and HR."
      />
      <ErrorNote error={sheets.error ?? settings.error} />
      {sheets.isLoading && <SkeletonList rows={3} label="Loading timesheets" />}
      {owner && settings.data && <PolicyForm key={JSON.stringify(settings.data)} ws={ws} settings={settings.data} />}
      {sheets.data &&
        (items.length === 0 ? (
          <Card>
            <EmptyState
              title="No billable people yet"
              description="Mark people as Billable in People, then name the HR mailbox above. Each month's timesheet status for them shows up here."
              action={
                <Link to={`/w/${ws}/people`}>
                  <Button variant="secondary">Go to People</Button>
                </Link>
              }
            />
          </Card>
        ) : (
          <div className="rise flex flex-col gap-2" style={riseStyle(1)}>
            <p className="text-sm text-ink-muted">
              {sheets.data.month}: {sheets.data.missing === 0 ? 'nothing is missing.' : `${sheets.data.missing} missing.`}
            </p>
            <Table>
              <thead>
                <tr>
                  <Th>Person</Th>
                  <Th>Status</Th>
                  <Th>Hours in the mail</Th>
                  <Th>Received</Th>
                </tr>
              </thead>
              <tbody>
                {items.map((r) => (
                  <tr key={r.person_id}>
                    <Td className="font-semibold text-ink">{r.name}</Td>
                    <Td>
                      <Badge tone={TONE[r.status]}>{LABEL[r.status]}</Badge>
                    </Td>
                    <Td>{r.timesheet?.hours ?? '–'}</Td>
                    <Td>{r.timesheet ? new Date(r.timesheet.received_at).toLocaleDateString('en-GB') : '–'}</Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          </div>
        ))}
    </Page>
  );
}
