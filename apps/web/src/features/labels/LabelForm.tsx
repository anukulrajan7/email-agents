import { useState, type FormEvent } from 'react';
import type { LabelColor } from '../../api/types';
import { Button, Input, Label as FieldLabel, Textarea, labelColors } from '../../components/ui';
import { cn } from '../../lib/cn';

export const COLORS: LabelColor[] = ['slate', 'sage', 'plum', 'ochre', 'teal', 'postmark', 'ink', 'rule'];
export const MIN_DESCRIPTION = 10;
export const MAX_DESCRIPTION = 300;

export type LabelFormValues = { name: string; description: string; color: LabelColor };

export function LabelForm({
  idPrefix,
  initial,
  submitLabel,
  busy,
  onSubmit,
  onCancel,
}: {
  idPrefix: string;
  initial?: LabelFormValues;
  submitLabel: string;
  busy: boolean;
  onSubmit: (values: LabelFormValues) => void;
  onCancel?: () => void;
}) {
  const [name, setName] = useState(initial?.name ?? '');
  const [description, setDescription] = useState(initial?.description ?? '');
  const [color, setColor] = useState<LabelColor>(initial?.color ?? 'slate');
  const trimmedName = name.trim();
  const trimmedDesc = description.trim();
  const descOk = trimmedDesc.length >= MIN_DESCRIPTION && trimmedDesc.length <= MAX_DESCRIPTION;
  const valid = trimmedName.length > 0 && descOk;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!valid || busy) return;
    onSubmit({ name: trimmedName, description: trimmedDesc, color });
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <div className="flex flex-col gap-1">
        <FieldLabel htmlFor={`${idPrefix}-name`}>Name</FieldLabel>
        <Input id={`${idPrefix}-name`} required maxLength={40} placeholder="Invoices" value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="flex flex-col gap-1">
        <FieldLabel htmlFor={`${idPrefix}-desc`}>Description</FieldLabel>
        <Textarea
          id={`${idPrefix}-desc`}
          rows={3}
          required
          aria-describedby={`${idPrefix}-desc-count`}
          placeholder="Example: Invoices, receipts and payment reminders from vendors. Not purchase orders."
          value={description}
          onChange={(e) => setDescription(e.target.value)}
        />
        <p
          id={`${idPrefix}-desc-count`}
          className={cn('text-xs', description.length > 0 && !descOk ? 'text-danger' : 'text-ink-muted')}
        >
          {trimmedDesc.length}/{MAX_DESCRIPTION} — write {MIN_DESCRIPTION} to {MAX_DESCRIPTION} characters
        </p>
      </div>
      <fieldset className="flex flex-col gap-2">
        <legend className="kicker">Colour</legend>
        <div className="flex flex-wrap gap-2">
          {COLORS.map((c) => (
            <label key={c} className="relative">
              <input
                type="radio"
                name={`${idPrefix}-color`}
                value={c}
                checked={color === c}
                onChange={() => setColor(c)}
                className="peer sr-only"
              />
              <span
                className={cn(
                  'block h-8 w-8 rounded-sm border-2 border-transparent',
                  'peer-checked:border-ink peer-checked:ring-2 peer-checked:ring-raised peer-checked:ring-inset',
                  'peer-focus-visible:outline-2 peer-focus-visible:outline-offset-2 peer-focus-visible:outline-accent',
                  labelColors[c]?.swatch,
                )}
              />
              <span className="sr-only">{c}</span>
            </label>
          ))}
        </div>
      </fieldset>
      <p className="rounded-sm border-l-[3px] border-rule-strong bg-neutral-tint px-3 py-2 text-sm text-ink" aria-live="polite">
        The AI will be told: “{trimmedName || 'Name'}: {trimmedDesc || 'description'}”
      </p>
      <div className="flex gap-2">
        <Button type="submit" loading={busy} disabled={!valid}>
          {submitLabel}
        </Button>
        {onCancel && (
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
        )}
      </div>
    </form>
  );
}
