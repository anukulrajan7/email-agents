import { useState } from 'react';
import { ApiError } from '../../api/client';
import type { LabelColor } from '../../api/types';
import { Button, Card, Spinner, Stamp } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useCreateLabel, useSuggestLabels, type LabelSuggestion } from './hooks';

const PALETTE: Record<LabelColor, [number, number, number]> = {
  slate: [0x34, 0x55, 0x7a],
  sage: [0x3f, 0x6b, 0x4e],
  plum: [0x6b, 0x3f, 0x6e],
  ochre: [0x9a, 0x6a, 0x12],
  teal: [0x22, 0x58, 0x5a],
  postmark: [0xb7, 0x3a, 0x26],
  ink: [0x1c, 0x1b, 0x18],
  rule: [0xb9, 0xb0, 0x9c],
};

export function nearestLabelColor(color: string): LabelColor | undefined {
  if (color in PALETTE) return color as LabelColor;
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(color.trim());
  if (!m) return undefined;
  const rgb = [m[1], m[2], m[3]].map((h) => parseInt(h!, 16));
  let best: LabelColor | undefined;
  let bestDistance = Infinity;
  for (const [name, p] of Object.entries(PALETTE) as [LabelColor, [number, number, number]][]) {
    const d = p.reduce((sum, v, i) => sum + (v - rgb[i]!) ** 2, 0);
    if (d < bestDistance) {
      best = name;
      bestDistance = d;
    }
  }
  return best;
}

function SuggestionCard({ ws, suggestion, onDone }: { ws: string; suggestion: LabelSuggestion; onDone: () => void }) {
  const create = useCreateLabel(ws);
  const color = nearestLabelColor(suggestion.color);
  const s = suggestion;
  return (
    <li>
      <Card className="flex h-full flex-col gap-3 bg-raised">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <Stamp name={s.name} color={color} />
          <span className="font-mono text-xs text-ink-muted">~{s.estimated_count} emails</span>
        </div>
        <p className="text-sm text-ink">{s.description}</p>
        {s.example_subjects.length > 0 && (
          <div>
            <p className="kicker">Examples</p>
            <ul className="mt-1 flex flex-col gap-0.5">
              {s.example_subjects.slice(0, 3).map((subject) => (
                <li key={subject} className="truncate text-sm text-ink-muted" title={subject}>
                  {subject}
                </li>
              ))}
            </ul>
          </div>
        )}
        <ErrorNote error={create.error} />
        <div className="mt-auto flex gap-2">
          <Button
            size="sm"
            aria-label={`Add ${s.name}`}
            loading={create.isPending}
            onClick={() => create.mutate({ name: s.name, description: s.description, color }, { onSuccess: onDone })}
          >
            Add
          </Button>
          <Button size="sm" variant="secondary" aria-label={`Dismiss ${s.name}`} disabled={create.isPending} onClick={onDone}>
            Dismiss
          </Button>
        </div>
      </Card>
    </li>
  );
}

export function SuggestLabels({ ws }: { ws: string }) {
  const suggest = useSuggestLabels(ws);
  const [gone, setGone] = useState<Set<string>>(new Set());
  const items = suggest.data?.items.filter((s) => !gone.has(s.key)) ?? [];
  const unavailable = suggest.error instanceof ApiError && suggest.error.code === 'ai_unavailable';

  const run = () => {
    setGone(new Set());
    suggest.mutate();
  };

  return (
    <section aria-labelledby="suggest-heading" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-3">
        <h2 id="suggest-heading" className="type-display text-2xl tracking-[-0.01em]">
          Suggested labels
        </h2>
        <Button variant="secondary" size="sm" onClick={run} loading={suggest.isPending}>
          {suggest.data ? 'Suggest again' : 'Suggest labels from my email'}
        </Button>
      </div>
      {suggest.isPending && (
        <p role="status" className="flex items-center gap-2 text-sm text-ink-muted">
          <Spinner className="h-4 w-4" /> Reading your recent mail…
        </p>
      )}
      {unavailable && (
        <div role="alert" className="flex flex-wrap items-center gap-3 rounded-sm border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
          <span>Suggestions are taking a break. Nothing is lost, so please try again in a moment.</span>
          <Button size="sm" variant="secondary" onClick={run}>
            Try again
          </Button>
        </div>
      )}
      {!unavailable && <ErrorNote error={suggest.error} />}
      {suggest.data && items.length === 0 && (
        <p role="status" className="text-sm text-ink-muted">
          {suggest.data.items.length === 0 ? 'No new labels to suggest right now. Your current labels cover your recent mail.' : 'All suggestions handled.'}
        </p>
      )}
      {items.length > 0 && (
        <ul className="grid gap-4 md:grid-cols-2 2xl:grid-cols-3" aria-label="Label suggestions">
          {items.map((s) => (
            <SuggestionCard key={s.key} ws={ws} suggestion={s} onDone={() => setGone((g) => new Set(g).add(s.key))} />
          ))}
        </ul>
      )}
    </section>
  );
}
