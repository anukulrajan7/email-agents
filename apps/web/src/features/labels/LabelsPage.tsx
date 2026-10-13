import { useEffect, useState } from 'react';
import type { Label } from '../../api/types';
import { Badge, Button, Card, CardTitle, EmptyState, Page, PageHeader, Spinner, Stamp } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { markLabelsVisited } from '../onboarding/storage';
import { LabelForm } from './LabelForm';
import { SuggestLabels } from './SuggestLabels';
import { useCreateLabel, useLabels, useUpdateLabel } from './hooks';

function LabelRow({
  ws,
  label,
  isAdmin,
  isFirst,
  isLast,
  onMove,
  onArchive,
  busy,
}: {
  ws: string;
  label: Label;
  isAdmin: boolean;
  isFirst: boolean;
  isLast: boolean;
  onMove: (direction: -1 | 1) => void;
  onArchive: (archived: boolean) => void;
  busy: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const update = useUpdateLabel(ws);

  if (editing) {
    return (
      <li className="p-4">
        <LabelForm
          idPrefix={`edit-${label.id}`}
          initial={{ name: label.name, description: label.description, color: label.color }}
          submitLabel="Save changes"
          busy={update.isPending}
          onCancel={() => setEditing(false)}
          onSubmit={(v) =>
            update.mutate(
              { id: label.id, ...v },
              {
                onSuccess: () => setEditing(false),
              },
            )
          }
        />
        <div className="mt-3">
          <ErrorNote error={update.error} />
        </div>
      </li>
    );
  }

  return (
    <li className={label.archived ? 'p-4 opacity-70' : 'p-4'}>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <Stamp name={label.name} color={label.color} />
            {label.system && <Badge tone="info">System</Badge>}
            {label.archived && <Badge>Archived</Badge>}
            <span className="font-mono text-xs text-ink-muted">
              {label.message_count} {label.message_count === 1 ? 'email' : 'emails'}
            </span>
          </div>
          <p className="max-w-prose text-sm text-ink">{label.description}</p>
        </div>
        {isAdmin && (
          <div className="flex flex-wrap items-center gap-2">
            <Button size="sm" variant="secondary" aria-label={`Move ${label.name} up`} disabled={busy || isFirst} onClick={() => onMove(-1)}>
              Up
            </Button>
            <Button size="sm" variant="secondary" aria-label={`Move ${label.name} down`} disabled={busy || isLast} onClick={() => onMove(1)}>
              Down
            </Button>
            <Button size="sm" variant="secondary" aria-label={`Edit ${label.name}`} onClick={() => setEditing(true)}>
              Edit
            </Button>
            <Button
              size="sm"
              variant="secondary"
              aria-label={`${label.archived ? 'Unarchive' : 'Archive'} ${label.name}`}
              title={label.system ? 'The system label cannot be archived' : undefined}
              disabled={busy || (label.system && !label.archived)}
              onClick={() => onArchive(!label.archived)}
            >
              {label.archived ? 'Unarchive' : 'Archive'}
            </Button>
          </div>
        )}
      </div>
    </li>
  );
}

export function LabelsPage() {
  const { ws, isAdmin } = useWorkspace();
  const labels = useLabels(ws, { includeArchived: true });
  const create = useCreateLabel(ws);
  const update = useUpdateLabel(ws);
  const [formKey, setFormKey] = useState(0);
  const items = labels.data?.items ?? [];

  useEffect(() => {
    markLabelsVisited(ws);
  }, [ws]);

  const move = (index: number, direction: -1 | 1) => {
    const target = items[index + direction];
    const label = items[index];
    if (target && label) update.mutate({ id: label.id, position: target.position });
  };

  return (
    <Page>
      <PageHeader
        kicker="How the AI sorts your mail"
        title="Labels"
        description="Each label has a description. The AI reads it as an instruction, and your team sees it as help text — write it like you'd brief a new teammate."
      />
      {labels.isLoading && <Spinner />}
      <ErrorNote error={labels.error ?? update.error} />
      {labels.data &&
        (items.length === 0 ? (
          <Card className="rise" style={riseStyle(1)}>
            <EmptyState
              title="No labels yet"
              description="Labels are the categories the AI sorts each email into. An admin can add the first one below."
            />
          </Card>
        ) : (
          <Card className="rise p-0" style={riseStyle(1)}>
            <ul className="divide-y divide-rule" aria-label="Labels">
              {items.map((l, i) => (
                <LabelRow
                  key={l.id}
                  ws={ws}
                  label={l}
                  isAdmin={isAdmin}
                  isFirst={i === 0}
                  isLast={i === items.length - 1}
                  busy={update.isPending}
                  onMove={(d) => move(i, d)}
                  onArchive={(archived) => update.mutate({ id: l.id, archived })}
                />
              ))}
            </ul>
          </Card>
        ))}
      {isAdmin && <SuggestLabels ws={ws} />}
      {isAdmin ? (
        <Card className="rise max-w-xl bg-raised" style={riseStyle(2)}>
          <CardTitle className="mb-3">Add a label</CardTitle>
          <LabelForm
            key={formKey}
            idPrefix="new-label"
            submitLabel="Add label"
            busy={create.isPending}
            onSubmit={(v) => create.mutate(v, { onSuccess: () => setFormKey((k) => k + 1) })}
          />
          <div className="mt-3">
            <ErrorNote error={create.error} />
          </div>
        </Card>
      ) : (
        <p className="text-sm text-ink-muted">Only workspace admins can add or change labels.</p>
      )}
    </Page>
  );
}
