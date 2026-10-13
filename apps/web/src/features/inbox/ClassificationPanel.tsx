import type { Classification, Priority } from '../../api/types';
import { Label, Select } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { humanize } from '../../lib/format';
import { LabelStamp } from '../labels/LabelStamp';
import { useLabelMap, useLabels } from '../labels/hooks';
import { useUpdateClassification } from './hooks';

const priorities: Priority[] = ['low', 'normal', 'high'];

function Explanation({ classification, labelName }: { classification: Classification; labelName: string }) {
  const strong = <strong>{labelName}</strong>;
  if (classification.source === 'manual') return <p>Labelled {strong} by you.</p>;
  if (classification.source === 'fallback') {
    return <p>This email was hard to read, so it was put in {strong}. Change the label if that is not right.</p>;
  }
  const pct = classification.confidence === null ? undefined : Math.round(classification.confidence * 100);
  return (
    <p>
      Labelled {strong} automatically{pct === undefined ? '' : ` (${pct}% sure)`}.
    </p>
  );
}

function Corrections({ ws, messageId, classification }: { ws: string; messageId: string; classification: Classification | null }) {
  const labels = useLabels(ws, { includeArchived: false });
  const update = useUpdateClassification(ws, messageId);
  const items = labels.data?.items ?? [];
  const category = classification?.category ?? '';
  const priority = classification?.priority ?? 'normal';

  return (
    <div className="mt-3 flex flex-wrap items-end gap-3">
      <div className="flex flex-col gap-1">
        <Label htmlFor="cls-category">Change label</Label>
        <Select
          id="cls-category"
          value={category}
          disabled={update.isPending}
          onChange={(e) => update.mutate({ category: e.target.value, priority })}
          className="w-48"
        >
          {category === '' && <option value="">Choose a label…</option>}
          {category !== '' && !items.some((l) => l.key === category) && <option value={category}>{category}</option>}
          {items.map((l) => (
            <option key={l.id} value={l.key}>
              {l.name}
            </option>
          ))}
        </Select>
      </div>
      {classification && (
        <div className="flex flex-col gap-1">
          <Label htmlFor="cls-priority">Priority</Label>
          <Select
            id="cls-priority"
            value={priority}
            disabled={update.isPending}
            onChange={(e) => update.mutate({ category: classification.category, priority: e.target.value as Priority })}
            className="w-32"
          >
            {priorities.map((p) => (
              <option key={p} value={p}>
                {humanize(p)}
              </option>
            ))}
          </Select>
        </div>
      )}
      <span role="status" className="min-h-5 text-sm text-success">
        {update.isSuccess ? 'Saved' : ''}
      </span>
      <div className="basis-full">
        <ErrorNote error={update.error} />
      </div>
    </div>
  );
}

export function ClassificationPanel({ ws, messageId, classification }: { ws: string; messageId: string; classification: Classification | null }) {
  const label = useLabelMap(ws).get(classification?.category ?? '');
  return (
    <section aria-labelledby="cls-heading">
      <h3 id="cls-heading" className="kicker">
        Label
      </h3>
      {classification ? (
        <div className="mt-3 flex flex-col gap-2 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <LabelStamp ws={ws} category={classification.category} />
          </div>
          <Explanation classification={classification} labelName={label?.name ?? classification.category} />
          {label && (
            <p className="text-ink-muted">
              <span className="font-semibold text-ink">What this label means:</span> {label.description}
            </p>
          )}
        </div>
      ) : (
        <p className="mt-2 text-sm text-ink-muted">Labelling this email…</p>
      )}
      <Corrections ws={ws} messageId={messageId} classification={classification} />
    </section>
  );
}
