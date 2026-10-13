import { useState } from 'react';
import type { Category, Classification, Priority } from '../../api/types';
import { Badge, Button, Label, Select, Stamp } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { humanize } from '../../lib/format';
import { useUpdateClassification } from './hooks';
import { categories } from './MessageList';

const priorities: Priority[] = ['low', 'normal', 'high'];

function Editor({ ws, messageId, classification }: { ws: string; messageId: string; classification: Classification | null }) {
  const [category, setCategory] = useState<Category>(classification?.category ?? 'general');
  const [priority, setPriority] = useState<Priority>(classification?.priority ?? 'normal');
  const update = useUpdateClassification(ws, messageId);

  return (
    <div className="mt-4 flex flex-wrap items-end gap-3">
      <div className="flex flex-col gap-1">
        <Label htmlFor="cls-category">Category</Label>
        <Select id="cls-category" value={category} onChange={(e) => setCategory(e.target.value as Category)} className="w-48">
          {categories.map((c) => (
            <option key={c} value={c}>
              {humanize(c)}
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

export function ClassificationPanel({ ws, messageId, classification }: { ws: string; messageId: string; classification: Classification | null }) {
  return (
    <section aria-labelledby="cls-heading">
      <h3 id="cls-heading" className="kicker">
        Classification
      </h3>
      {classification ? (
        <div className="mt-3 flex flex-wrap items-center gap-2 text-sm">
          <Stamp category={classification.category} />
          <Badge>Source: {classification.source}</Badge>
          <Badge>
            Confidence: {classification.confidence === null ? 'n/a' : `${Math.round(classification.confidence * 100)}%`}
          </Badge>
          {classification.needs_review && <Badge tone="warning">Needs review</Badge>}
        </div>
      ) : (
        <p className="mt-2 text-sm text-ink-muted">Not classified yet. You can set a category manually.</p>
      )}
      <Editor key={classification?.updated_at ?? 'none'} ws={ws} messageId={messageId} classification={classification} />
    </section>
  );
}
