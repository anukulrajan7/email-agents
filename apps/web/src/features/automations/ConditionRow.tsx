import type { Label } from '../../api/types';
import { Button, Input, Select } from '../../components/ui';
import type { Condition, ConditionField } from './types';

export const FIELD_OPTIONS: { value: ConditionField; label: string }[] = [
  { value: 'label', label: 'Label' },
  { value: 'priority', label: 'Priority' },
  { value: 'needs_reply', label: 'Needs a reply' },
  { value: 'from', label: 'Sender' },
  { value: 'subject', label: 'Subject' },
];

export function defaultCondition(field: ConditionField, labels: Label[]): Condition {
  switch (field) {
    case 'label':
      return { field, op: 'is', value: labels[0]?.key ?? '' };
    case 'priority':
      return { field, op: 'is', value: 'high' };
    case 'needs_reply':
      return { field, op: 'is', value: true };
    case 'from':
    case 'subject':
      return { field, op: 'contains', value: '' };
  }
}

function ValueInput({ id, condition, labels, onChange }: { id: string; condition: Condition; labels: Label[]; onChange: (c: Condition) => void }) {
  const aria = { 'aria-label': `${id} value` };
  switch (condition.field) {
    case 'label': {
      const known = labels.some((l) => l.key === condition.value);
      return (
        <Select {...aria} className="w-48" value={condition.value} onChange={(e) => onChange({ ...condition, value: e.target.value })}>
          {labels.length === 0 && <option value="">No labels yet</option>}
          {!known && condition.value !== '' && <option value={condition.value}>{condition.value}</option>}
          {labels.map((l) => (
            <option key={l.id} value={l.key}>
              {l.name}
            </option>
          ))}
        </Select>
      );
    }
    case 'priority':
      return (
        <Select {...aria} className="w-36" value={condition.value} onChange={(e) => onChange({ ...condition, value: e.target.value as typeof condition.value })}>
          <option value="high">High</option>
          <option value="normal">Normal</option>
          <option value="low">Low</option>
        </Select>
      );
    case 'needs_reply':
      return (
        <Select {...aria} className="w-36" value={condition.value ? 'yes' : 'no'} onChange={(e) => onChange({ ...condition, value: e.target.value === 'yes' })}>
          <option value="yes">Yes</option>
          <option value="no">No</option>
        </Select>
      );
    case 'from':
    case 'subject':
      return (
        <Input
          {...aria}
          className="w-56"
          placeholder={condition.field === 'from' ? 'name or address' : 'word or phrase'}
          value={condition.value}
          onChange={(e) => onChange({ ...condition, value: e.target.value })}
        />
      );
  }
}

export function ConditionRow({
  index,
  condition,
  labels,
  onChange,
  onRemove,
}: {
  index: number;
  condition: Condition;
  labels: Label[];
  onChange: (c: Condition) => void;
  onRemove: () => void;
}) {
  const id = `Condition ${index + 1}`;
  return (
    <li className="flex flex-wrap items-center gap-2">
      <span className="w-10 text-sm text-ink-muted">{index === 0 ? '' : 'and'}</span>
      <Select
        aria-label={`${id} field`}
        className="w-44"
        value={condition.field}
        onChange={(e) => onChange(defaultCondition(e.target.value as ConditionField, labels))}
      >
        {FIELD_OPTIONS.map((o) => (
          <option key={o.value} value={o.value}>
            {o.label}
          </option>
        ))}
      </Select>
      {condition.field === 'label' ? (
        <Select
          aria-label={`${id} operator`}
          className="w-28"
          value={condition.op}
          onChange={(e) => onChange({ ...condition, op: e.target.value as 'is' | 'is_not' })}
        >
          <option value="is">is</option>
          <option value="is_not">is not</option>
        </Select>
      ) : (
        <span className="text-sm text-ink">{condition.op === 'contains' ? 'contains' : 'is'}</span>
      )}
      <ValueInput id={id} condition={condition} labels={labels} onChange={onChange} />
      <Button variant="ghost" size="sm" aria-label={`Remove ${id.toLowerCase()}`} onClick={onRemove}>
        Remove
      </Button>
    </li>
  );
}
