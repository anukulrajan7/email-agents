import { useState } from 'react';
import type { Classification, Priority } from '../../api/types';
import { Badge, Button, Label, Select } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { humanize } from '../../lib/format';
import { LabelStamp } from '../labels/LabelStamp';
import { useLabelMap, useLabels } from '../labels/hooks';
import { useApproveClassification, useUpdateClassification } from './hooks';

const priorities: Priority[] = ['low', 'normal', 'high'];

function Editor({ ws, messageId, classification }: { ws: string; messageId: string; classification: Classification | null }) {
  const labels = useLabels(ws, { includeArchived: false });
  const [category, setCategory] = useState<string>(classification?.category ?? 'other');
  const [priority, setPriority] = useState<Priority>(classification?.priority ?? 'normal');
  const update = useUpdateClassification(ws, messageId);
  const items = labels.data?.items ?? [];

  return (
    <div className="mt-3 flex flex-wrap items-end gap-3">
      <div className="flex flex-col gap-1">
        <Label htmlFor="cls-category">Label</Label>
        <Select id="cls-category" value={category} onChange={(e) => setCategory(e.target.value)} className="w-48">
          {items.length > 0 && !items.some((l) => l.key === category) && <option value={category}>{category}</option>}
          {items.map((l) => (
            <option key={l.id} value={l.key}>
              {l.name}
            </option>
          ))}
        </Select>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="cls-priority">Priority</Label>
        <Select id="cls-priority" value={priority} onChange={(e) => setPriority(e.target.value as Priority)} className="w-32">
          {priorities.map((p) => (
            <option key={p} value={p}>
              {humanize(p)}
            </option>
          ))}
        </Select>
      </div>
      <Button loading={update.isPending} onClick={() => update.mutate({ category, priority })}>
        Save
      </Button>
      {update.isSuccess && <span className="text-sm text-success">Saved</span>}
      <div className="basis-full">
        <ErrorNote error={update.error} />
      </div>
    </div>
  );
}

function Explanation({ classification, labelName }: { classification: Classification; labelName: string }) {
  const pct = classification.confidence === null ? undefined : Math.round(classification.confidence * 100);
  const strong = <strong>{labelName}</strong>;
  if (classification.source === 'manual') {
    return <p>Labelled {strong} by you.</p>;
  }
  if (classification.source === 'llm' || classification.source === 'jev') {
    const detail = [classification.model_id, pct === undefined ? undefined : `${pct}% sure`].filter(Boolean).join(', ');
    return (
      <p>
        Labelled {strong} by AI{detail ? ` (${detail})` : ''}.
        {classification.needs_review && " Needs review because the AI wasn't confident enough."}
      </p>
    );
  }
  if (classification.model_id === 'error-fallback') {
    return <p>The AI couldn&apos;t label this email after several tries, so it was put in {strong} for a person to check.</p>;
  }
  return (
    <p>
      Labelled {strong} by keyword rules (no AI configured) — please check.
      {classification.needs_review && ' Needs review.'}
    </p>
  );
}

function Approve({ approve }: { approve: ReturnType<typeof useApproveClassification> }) {
  return (
    <div className="flex flex-wrap items-center gap-3">
      {approve.isSuccess ? (
        <span role="status" className="text-sm font-semibold text-success">
          Approved ✓
        </span>
      ) : (
        <Button loading={approve.isPending} onClick={() => approve.mutate()}>
          Approve label
        </Button>
      )}
      <div className="basis-full">
        <ErrorNote error={approve.error} />
      </div>
    </div>
  );
}

export function ClassificationPanel({ ws, messageId, classification }: { ws: string; messageId: string; classification: Classification | null }) {
  const label = useLabelMap(ws).get(classification?.category ?? '');
  const [editing, setEditing] = useState(false);
  const approve = useApproveClassification(ws, messageId);
  const canApprove = classification?.needs_review === true && classification.source !== 'manual';
  return (
    <section aria-labelledby="cls-heading">
      <h3 id="cls-heading" className="kicker">
        Label
      </h3>
      {classification ? (
        <div className="mt-3 flex flex-col gap-2 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <LabelStamp ws={ws} category={classification.category} />
            {classification.needs_review && <Badge tone="warning">Needs review</Badge>}
          </div>
          <Explanation classification={classification} labelName={label?.name ?? classification.category} />
          {label && (
            <p className="text-ink-muted">
              <span className="font-semibold text-ink">What this label means:</span> {label.description}
            </p>
          )}
        </div>
      ) : (
        <p className="mt-2 text-sm text-ink-muted">Not labelled yet. The AI labels each email shortly after it arrives, or you can pick a label yourself.</p>
      )}
      {(canApprove || approve.isSuccess) && (
        <div className="mt-3">
          <Approve approve={approve} />
        </div>
      )}
      <div className="mt-3">
        <Button
          variant="ghost"
          size="sm"
          className="-ml-3 underline underline-offset-2"
          aria-expanded={editing}
          aria-controls="cls-editor"
          onClick={() => setEditing((v) => !v)}
        >
          {editing ? 'Hide label options' : 'Change label'}
        </Button>
      </div>
      {editing && (
        <div id="cls-editor">
          <Editor key={classification?.updated_at ?? 'none'} ws={ws} messageId={messageId} classification={classification} />
        </div>
      )}
    </section>
  );
}
