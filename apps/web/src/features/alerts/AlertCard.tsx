import { useId } from 'react';
import { Link } from 'react-router';
import { Badge, Button, Card, Mono, RelativeTime, Select } from '../../components/ui';
import type { Alert } from './types';
import { hoursUntilTomorrowMorning } from './validation';

const SNOOZE_OPTIONS = [
  { value: '1', label: '1 hour' },
  { value: '4', label: '4 hours' },
  { value: 'tomorrow', label: 'Tomorrow' },
];

export function AlertCard({
  ws,
  alert,
  busy,
  onResolve,
  onReopen,
  onSnooze,
}: {
  ws: string;
  alert: Alert;
  busy: boolean;
  onResolve: () => void;
  onReopen: () => void;
  onSnooze: (hours: number) => void;
}) {
  const uid = useId();
  const resolved = alert.status === 'resolved';
  const snoozed = alert.status === 'snoozed';

  return (
    <li>
      <Card className="flex flex-col gap-3" aria-labelledby={`${uid}-title`} role="group">
        <div className="flex flex-wrap items-center gap-2">
          <Badge tone={alert.severity === 'high' ? 'danger' : 'neutral'}>{alert.severity === 'high' ? 'High' : 'Normal'}</Badge>
          {alert.escalation_level > 0 && <Badge tone="warning">Escalated to manager</Badge>}
          {snoozed && <Badge tone="info">Snoozed</Badge>}
          <Mono className="ml-auto">
            <RelativeTime iso={alert.created_at} />
          </Mono>
        </div>
        <div>
          <h3 id={`${uid}-title`} className="text-[15px] font-semibold text-ink">
            {alert.title}
          </h3>
          <p className="mt-1 text-sm break-words whitespace-pre-wrap text-ink-muted">{alert.body}</p>
        </div>
        <div className="flex flex-col gap-1 text-sm text-ink-muted">
          {alert.message_id && (
            <p>
              Email:{' '}
              <Link to={`/w/${ws}/inbox/${alert.message_id}`} className="rounded-xs text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover">
                {alert.message_subject || '(no subject)'}
              </Link>
            </p>
          )}
          {alert.notified.length > 0 && <p className="break-words">Notified: {alert.notified.join(', ')}</p>}
          {resolved && alert.resolved_at && (
            <p>
              Resolved <RelativeTime iso={alert.resolved_at} />
            </p>
          )}
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {!resolved && (
            <Button size="sm" disabled={busy} onClick={onResolve}>
              Resolve
            </Button>
          )}
          {!resolved && (
            <Select
              aria-label={`Snooze: ${alert.title}`}
              className="h-8 w-32 text-[13px]"
              disabled={busy}
              value=""
              onChange={(e) => {
                const v = e.target.value;
                if (v) onSnooze(v === 'tomorrow' ? hoursUntilTomorrowMorning() : Number(v));
              }}
            >
              <option value="">Snooze…</option>
              {SNOOZE_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </Select>
          )}
          {(resolved || snoozed) && (
            <Button size="sm" variant="secondary" disabled={busy} onClick={onReopen}>
              Reopen
            </Button>
          )}
        </div>
      </Card>
    </li>
  );
}
