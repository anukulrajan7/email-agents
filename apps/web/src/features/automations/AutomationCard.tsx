import { Button, Card, CardTitle, RelativeTime, Switch } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { AutomationSentence } from './Sentence';
import type { Automation } from './types';

export function AutomationCard({
  automation,
  index,
  isAdmin,
  togglePending,
  labelName,
  onToggle,
  onRuns,
  onEdit,
  onDelete,
}: {
  automation: Automation;
  index: number;
  isAdmin: boolean;
  togglePending: boolean;
  labelName: (key: string) => string;
  onToggle: (enabled: boolean) => void;
  onRuns: () => void;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const a = automation;
  return (
    <li>
      <Card className="rise flex flex-col gap-3" style={riseStyle(index + 1)}>
        <div className="flex flex-wrap items-start justify-between gap-3">
          <CardTitle>{a.name}</CardTitle>
          <div className="flex items-center gap-2 text-sm">
            <Switch checked={a.enabled} label={`${a.name} enabled`} disabled={!isAdmin || togglePending} onChange={onToggle} />
            <span aria-hidden="true" className="w-8 text-ink-muted">{a.enabled ? 'On' : 'Off'}</span>
          </div>
        </div>
        <AutomationSentence conditions={a.conditions} actions={a.actions} labelName={labelName} />
        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-rule pt-3">
          <p className="font-mono text-xs text-ink-muted">
            {a.run_count === 0 ? 'Has not run yet' : `Ran ${a.run_count} ${a.run_count === 1 ? 'time' : 'times'}`}
            {a.last_run_at && (
              <>
                {' · last '}
                <RelativeTime iso={a.last_run_at} />
              </>
            )}
          </p>
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="secondary" aria-label={`Runs for ${a.name}`} onClick={onRuns}>
              Runs
            </Button>
            {isAdmin && (
              <>
                <Button size="sm" variant="secondary" aria-label={`Edit ${a.name}`} onClick={onEdit}>
                  Edit
                </Button>
                <Button size="sm" variant="danger" aria-label={`Delete ${a.name}`} onClick={onDelete}>
                  Delete
                </Button>
              </>
            )}
          </div>
        </div>
      </Card>
    </li>
  );
}
