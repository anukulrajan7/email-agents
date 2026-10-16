import { useId, useState, type FormEvent } from 'react';
import { Button, Card, Input, Label, Select, Switch, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useSaveTeamSettings } from '../team/hooks';
import type { TeamSettings, TimesheetCadence } from '../team/types';

const CADENCES: { value: TimesheetCadence; label: string }[] = [
  { value: 'monthly', label: 'Once a month' },
  { value: 'weekly', label: 'Every week' },
  { value: 'daily', label: 'Every day' },
];

export function TimesheetRulesForm({ ws, settings }: { ws: string; settings: TeamSettings }) {
  const uid = useId();
  const save = useSaveTeamSettings(ws);
  const notify = useToast();
  const [cadence, setCadence] = useState<TimesheetCadence>(settings.timesheet_cadence ?? 'monthly');
  const [maxHours, setMaxHours] = useState(settings.max_hours_per_day == null ? '' : String(settings.max_hours_per_day));
  const [tolerance, setTolerance] = useState(String(settings.hours_tolerance ?? 0.5));
  const [problem, setProblem] = useState<string>();
  const [signature, setSignature] = useState(settings.require_signature ?? false);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const tol = Number(tolerance);
    if (tolerance.trim() === '' || !Number.isFinite(tol) || tol < 0 || tol > 100) {
      setProblem('Enter how many hours a sheet may differ from the portal, from 0 to 100.');
      return;
    }
    setProblem(undefined);
    save.mutate(
      {
        ...settings,
        timesheet_cadence: cadence,
        max_hours_per_day: maxHours.trim() === '' ? null : Number(maxHours),
        hours_tolerance: tol,
        require_signature: signature,
      },
      { onSuccess: () => notify('success', 'Timesheet rules saved.') },
    );
  };

  return (
    <Card>
      <form onSubmit={submit} className="flex flex-col gap-4">
        <div>
          <h2 className="text-base font-semibold text-ink">Timesheet rules</h2>
          <p className="mt-1 text-sm text-ink-muted">
            Rohan checks every timesheet that arrives against these rules. A sheet that breaks one is marked for a check, and the sender is not told yet. When a sheet is fine, its manager gets a short summary.
          </p>
        </div>
        <div className="flex flex-wrap gap-6">
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-cadence`}>How often a timesheet is expected</Label>
            <Select id={`${uid}-cadence`} value={cadence} onChange={(e) => setCadence(e.target.value as TimesheetCadence)}>
              {CADENCES.map((c) => (
                <option key={c.value} value={c.value}>
                  {c.label}
                </option>
              ))}
            </Select>
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-max`}>Most hours in one day</Label>
            <Input id={`${uid}-max`} type="number" min={0.5} max={24} step={0.5} className="w-28" placeholder="No limit" value={maxHours} onChange={(e) => setMaxHours(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-tol`}>Hours a sheet may differ from the portal</Label>
            <Input id={`${uid}-tol`} type="number" min={0} max={100} step={0.5} className="w-28" value={tolerance} onChange={(e) => setTolerance(e.target.value)} />
          </div>
        </div>
        <div className="flex items-center gap-3 text-sm text-ink">
          <Switch checked={signature} label="Timesheets need an approver's signature" describedBy={`${uid}-signature-note`} onChange={setSignature} />
          <span aria-hidden="true">Timesheets need an approver's signature</span>
          <span id={`${uid}-signature-note`} className="text-ink-muted">
            (coming soon: we can't detect signatures yet, so this is not enforced)
          </span>
        </div>
        {problem && <p role="alert" className="text-sm text-danger">{problem}</p>}
        <ErrorNote error={save.error} />
        <div>
          <Button type="submit" loading={save.isPending}>
            Save rules
          </Button>
        </div>
      </form>
    </Card>
  );
}
