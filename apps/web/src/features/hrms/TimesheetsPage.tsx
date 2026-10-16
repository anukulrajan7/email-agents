import { useId, useState, type FormEvent } from 'react';
import { useSearchParams } from 'react-router';
import { Button, Card, Dialog, Input, Label, Page, PageHeader, Select, Tabs, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { useSaveTeamSettings, useTeamAccess, useTeamSettings } from '../team/hooks';
import type { TeamSettings } from '../team/types';
import { RecordById } from './RecordDetail';
import { TimesheetGridView } from './TimesheetGridView';
import { TimesheetRulesForm } from './TimesheetRulesForm';
import { TimesheetRecordsView } from './TimesheetRecordsView';
import type { GridRow } from './types';

function PolicyForm({ ws, settings }: { ws: string; settings: TeamSettings }) {
  const uid = useId();
  const save = useSaveTeamSettings(ws);
  const notify = useToast();
  const [hr, setHr] = useState(settings.hr_source_id ?? '');
  const [grace, setGrace] = useState(settings.timesheet_grace_days ?? 3);
  const [wall, setWall] = useState(settings.restrict_mail_to_team ?? false);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate({ ...settings, hr_source_id: hr || null, timesheet_grace_days: grace, restrict_mail_to_team: wall }, { onSuccess: () => notify('success', 'Billable policy saved.') });
  };

  return (
    <Card>
      <form onSubmit={submit} className="flex flex-col gap-4">
        <p className="text-sm text-ink-muted">
          Billable people's mail is read only when it arrives through the company HR mailbox. Pick that mailbox here; mail sent by a billable person outside that mailbox is held back: kept safely, but not shown or read by the AI until the policy allows it.
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
        <label className="flex items-center gap-2 text-sm text-ink">
          <input type="checkbox" checked={wall} onChange={(e) => setWall(e.target.checked)} />
          Members read only the mailboxes of their own team
        </label>
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

type View = 'grid' | 'records';

export function TimesheetsPage() {
  const { ws } = useWorkspace();
  const access = useTeamAccess(ws);
  const settings = useTeamSettings(ws);
  const owner = access.data?.level === 'super_admin';
  const [params, setParams] = useSearchParams();
  const [view, setView] = useState<View>(params.get('record') ? 'records' : 'grid');
  const [row, setRow] = useState<GridRow>();
  const recordParam = params.get('record') ?? undefined;
  const closeRecord = () => setParams((p) => { p.delete('record'); return p; }, { replace: true });

  return (
    <Page className="gap-6">
      <PageHeader
        kicker="HRMS · Timesheets"
        title="Timesheets"
        description="Timesheets that arrive, pasted in an email or attached, are read and kept here by person and day. If one has not arrived three days after month end, the person is told and then their manager and HR."
      />
      <ErrorNote error={settings.error} />
      <Tabs
        label="Timesheet views"
        value={view}
        onChange={setView}
        tabs={[
          { id: 'grid', label: 'Team grid' },
          { id: 'records', label: 'Records' },
        ]}
      >
        {view === 'grid' ? <TimesheetGridView ws={ws} onOpen={setRow} /> : <TimesheetRecordsView ws={ws} canLink={owner} />}
      </Tabs>
      {row && (
        <Dialog title={`${row.name}'s timesheets`} variant="drawer" onClose={() => setRow(undefined)}>
          <div className="flex flex-col gap-6">
            {row.record_ids.map((id) => (
              <RecordById key={id} ws={ws} id={id} canLink={owner} />
            ))}
          </div>
        </Dialog>
      )}
      {recordParam && (
        <Dialog title="Timesheet record" variant="drawer" onClose={closeRecord}>
          <RecordById ws={ws} id={recordParam} canLink={owner} />
        </Dialog>
      )}
      {owner && settings.data && <PolicyForm key={JSON.stringify([settings.data.hr_source_id, settings.data.timesheet_grace_days, settings.data.restrict_mail_to_team])} ws={ws} settings={settings.data} />}
      {owner && settings.data && <TimesheetRulesForm key={JSON.stringify([settings.data.timesheet_cadence, settings.data.max_hours_per_day, settings.data.hours_tolerance, settings.data.require_signature])} ws={ws} settings={settings.data} />}
    </Page>
  );
}
