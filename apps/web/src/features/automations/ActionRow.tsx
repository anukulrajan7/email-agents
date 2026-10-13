import { Badge, Button, Card, Input, Label as FieldLabel, Textarea } from '../../components/ui';
import type { Action, ActionType } from './types';
import { DomainChips } from './DomainChips';
import { DEFAULT_DAILY_CAP, DEFAULT_REMIND_HOURS } from './validate';

export const ACTION_OPTIONS: { type: ActionType; label: string }[] = [
  { type: 'draft_reply', label: 'Draft a reply' },
  { type: 'save_to_gmail_drafts', label: 'Save to Gmail drafts' },
  { type: 'send_reply', label: 'Send the reply' },
  { type: 'create_review', label: 'Create a review task' },
  { type: 'remind_if_no_reply', label: 'Remind me if no reply' },
];

export const SEND_WARNING = 'Sends automatically. Skipped when the draft has missing info [NEEDS] or the daily cap is reached.';

export function defaultAction(type: ActionType): Action {
  switch (type) {
    case 'draft_reply':
    case 'save_to_gmail_drafts':
    case 'create_review':
      return { type };
    case 'send_reply':
      return { type, daily_cap: DEFAULT_DAILY_CAP, allowed_domains: [] };
    case 'remind_if_no_reply':
      return { type, hours: DEFAULT_REMIND_HOURS };
  }
}

const toInt = (value: string) => (value === '' ? Number.NaN : Number(value));

function ActionFields({ id, action, onChange }: { id: string; action: Action; onChange: (a: Action) => void }) {
  switch (action.type) {
    case 'draft_reply':
      return (
        <div className="flex flex-col gap-1">
          <FieldLabel htmlFor={`${id}-instructions`}>Extra guidance (optional)</FieldLabel>
          <Textarea
            id={`${id}-instructions`}
            rows={2}
            placeholder="Example: Keep it to two sentences and thank them for the timesheet."
            value={action.instructions ?? ''}
            onChange={(e) => onChange({ ...action, instructions: e.target.value })}
          />
        </div>
      );
    case 'save_to_gmail_drafts':
      return <p className="text-sm text-ink-muted">Puts the draft in the Drafts folder of the connected Gmail account.</p>;
    case 'send_reply':
      return (
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2 text-sm">
            <FieldLabel htmlFor={`${id}-cap`}>Daily cap</FieldLabel>
            <Input
              id={`${id}-cap`}
              type="number"
              min={1}
              step={1}
              className="w-24"
              value={Number.isNaN(action.daily_cap) ? '' : action.daily_cap}
              onChange={(e) => onChange({ ...action, daily_cap: toInt(e.target.value) })}
            />
            <span className="text-ink-muted">replies a day at most</span>
          </div>
          <DomainChips id={`${id}-domains`} domains={action.allowed_domains} onChange={(allowed_domains) => onChange({ ...action, allowed_domains })} />
          <p className="rounded-sm border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">{SEND_WARNING}</p>
        </div>
      );
    case 'create_review':
      return (
        <div className="flex flex-col gap-1">
          <FieldLabel htmlFor={`${id}-title`}>Task title (optional)</FieldLabel>
          <Input
            id={`${id}-title`}
            placeholder="Example: Check this timesheet"
            value={action.title ?? ''}
            onChange={(e) => onChange({ ...action, title: e.target.value })}
          />
        </div>
      );
    case 'remind_if_no_reply':
      return (
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <FieldLabel htmlFor={`${id}-hours`}>Remind after</FieldLabel>
          <Input
            id={`${id}-hours`}
            type="number"
            min={1}
            step={1}
            className="w-24"
            value={Number.isNaN(action.hours) ? '' : action.hours}
            onChange={(e) => onChange({ ...action, hours: toInt(e.target.value) })}
          />
          <span className="text-ink-muted">hours if the thread is still unanswered</span>
        </div>
      );
  }
}

export function ActionRow({
  actionId,
  index,
  count,
  action,
  problem,
  onChange,
  onMove,
  onRemove,
}: {
  actionId: number;
  index: number;
  count: number;
  action: Action;
  problem: string | undefined;
  onChange: (a: Action) => void;
  onMove: (direction: -1 | 1) => void;
  onRemove: () => void;
}) {
  const step = index + 1;
  const title = ACTION_OPTIONS.find((o) => o.type === action.type)?.label ?? action.type;
  return (
    <li data-action-id={actionId}>
      <Card className="flex flex-col gap-3 bg-raised">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <span aria-hidden="true" className="flex h-6 w-6 items-center justify-center rounded-full border border-rule-strong text-xs font-semibold text-ink-muted">
              {step}
            </span>
            <h4 className="text-sm font-semibold text-ink">{title}</h4>
            {action.type === 'send_reply' && <Badge tone="warning">Automatic</Badge>}
          </div>
          <div className="flex items-center gap-1">
            <Button size="sm" variant="secondary" data-move="up" aria-label={`Move step ${step} up`} disabled={index === 0} onClick={() => onMove(-1)}>
              Up
            </Button>
            <Button size="sm" variant="secondary" data-move="down" aria-label={`Move step ${step} down`} disabled={index === count - 1} onClick={() => onMove(1)}>
              Down
            </Button>
            <Button size="sm" variant="ghost" aria-label={`Remove step ${step}`} onClick={onRemove}>
              Remove
            </Button>
          </div>
        </div>
        <ActionFields id={`action-${actionId}`} action={action} onChange={onChange} />
        {problem && (
          <p className="text-sm text-danger" role="note">
            {problem}
          </p>
        )}
      </Card>
    </li>
  );
}
