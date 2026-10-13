import { useId, useState, type FormEvent, type ReactNode } from 'react';
import { Button, Card, CardTitle, Input, Label, Select, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { EmailChipInput } from './EmailChipInput';
import { useAlertSettings, useSaveAlertSettings, useSendTestAlert } from './hooks';
import type { AlertSettings } from './types';
import { validateSettings } from './validation';

function Switch({ checked, label, onChange }: { checked: boolean; label: string; onChange: (next: boolean) => void }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
      className={cn(
        'relative inline-flex h-6 w-11 shrink-0 items-center rounded-full transition-colors',
        'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
        checked ? 'bg-success' : 'bg-ink-muted',
      )}
    >
      <span className={cn('inline-block h-5 w-5 rounded-full bg-white shadow-sm transition-transform', checked ? 'translate-x-5' : 'translate-x-0.5')} />
    </button>
  );
}

function HoursField({ label, before, after, value, error, onChange }: {
  label: string;
  before: string;
  after: string;
  value: number;
  error: string | undefined;
  onChange: (n: number) => void;
}) {
  const uid = useId();
  return (
    <div className="flex flex-col gap-1">
      <Label htmlFor={`${uid}-hours`}>{label}</Label>
      <div className="flex flex-wrap items-center gap-2 text-sm text-ink">
        <span aria-hidden="true">{before}</span>
        <Input
          id={`${uid}-hours`}
          type="number"
          min={1}
          max={720}
          step={1}
          className="w-24"
          value={Number.isNaN(value) ? '' : value}
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? `${uid}-error` : undefined}
          onChange={(e) => onChange(e.target.value === '' ? Number.NaN : Number(e.target.value))}
        />
        <span aria-hidden="true">{after}</span>
      </div>
      {error && (
        <p id={`${uid}-error`} role="alert" className="text-sm text-danger">
          {error}
        </p>
      )}
    </div>
  );
}

function Row({ title, description, control }: { title: string; description: string; control: ReactNode }) {
  return (
    <div className="flex items-center justify-between gap-4">
      <div>
        <p className="text-[15px] font-semibold text-ink">{title}</p>
        <p className="text-sm text-ink-muted">{description}</p>
      </div>
      {control}
    </div>
  );
}

function SettingsForm({ ws, initial }: { ws: string; initial: AlertSettings }) {
  const uid = useId();
  const [saved, setSaved] = useState(initial);
  const [form, setForm] = useState(initial);
  const [showErrors, setShowErrors] = useState(false);
  const save = useSaveAlertSettings(ws);
  const test = useSendTestAlert(ws);

  const set = <K extends keyof AlertSettings>(key: K, value: AlertSettings[K]) => {
    setForm((f) => ({ ...f, [key]: value }));
    save.reset();
  };
  const errors = validateSettings(form);
  const shown = showErrors ? errors : {};
  const dirty = JSON.stringify(form) !== JSON.stringify(saved);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setShowErrors(true);
    if (Object.keys(errors).length > 0) return;
    save.mutate(form, {
      onSuccess: () => {
        setSaved(form);
        setShowErrors(false);
      },
    });
  };

  return (
    <form onSubmit={submit} noValidate className="flex flex-col gap-6">
      <Card className="flex flex-col gap-6">
        <Row
          title="Email alerts"
          description="Tell people by email when something important needs attention."
          control={<Switch checked={form.enabled} label="Email alerts enabled" onChange={(v) => set('enabled', v)} />}
        />
        <EmailChipInput
          label="Send alerts to"
          hint="Press Enter or comma after each address."
          value={form.recipients}
          onChange={(v) => set('recipients', v)}
          error={shown.recipients}
        />
        <EmailChipInput
          label="Escalate to"
          hint="If nobody acts in time, these people are told next, such as a manager."
          value={form.escalate_to}
          onChange={(v) => set('escalate_to', v)}
        />
      </Card>

      <Card className="flex flex-col gap-6">
        <CardTitle>When to alert</CardTitle>
        <HoursField
          label="Hours until alert for an unanswered important email"
          before="Alert when an important email has no reply after"
          after="hours"
          value={form.unanswered_hours}
          error={shown.unanswered_hours}
          onChange={(n) => set('unanswered_hours', n)}
        />
        <HoursField
          label="Hours until escalation"
          before="Escalate after"
          after="hours without action"
          value={form.escalate_after_hours}
          error={shown.escalate_after_hours}
          onChange={(n) => set('escalate_after_hours', n)}
        />
        <Row
          title="Daily digest"
          description="One summary email of everything still open."
          control={<Switch checked={form.daily_digest} label="Daily digest enabled" onChange={(v) => set('daily_digest', v)} />}
        />
        {form.daily_digest && (
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-hour`}>Send digest at</Label>
            <Select id={`${uid}-hour`} className="w-32" value={form.digest_hour} onChange={(e) => set('digest_hour', Number(e.target.value))}>
              {Array.from({ length: 24 }, (_, h) => (
                <option key={h} value={h}>
                  {String(h).padStart(2, '0')}:00
                </option>
              ))}
            </Select>
          </div>
        )}
      </Card>

      <ErrorNote error={save.error ?? test.error} />
      <div className="flex flex-wrap items-center gap-3">
        <Button type="submit" loading={save.isPending}>
          Save settings
        </Button>
        <Button type="button" variant="secondary" loading={test.isPending} disabled={dirty} onClick={() => test.mutate()}>
          Send test alert
        </Button>
        <p role="status" className="text-sm text-ink-muted">
          {save.isSuccess && !dirty && 'Saved.'}
          {!save.isSuccess && dirty && 'Save your changes before sending a test.'}
          {test.data && !dirty && ` Sent to ${test.data.sent_to.join(', ')}`}
        </p>
      </div>
    </form>
  );
}

export function AlertSettingsPanel({ ws }: { ws: string }) {
  const settings = useAlertSettings(ws);
  return (
    <div className="flex max-w-2xl flex-col gap-4">
      {settings.isLoading && <Spinner />}
      <ErrorNote error={settings.error} />
      {settings.data && <SettingsForm ws={ws} initial={settings.data} />}
    </div>
  );
}
