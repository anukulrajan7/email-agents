import { useState } from 'react';
import { Button, Card, EmptyState, Page, PageHeader, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { useLabels } from '../labels/hooks';
import { AutomationBuilder } from './Builder';
import { AutomationCard } from './AutomationCard';
import { Overlay } from './Overlay';
import { PresetsGallery } from './PresetsGallery';
import { RunsDrawer } from './RunsDrawer';
import { useAutomations, useCreateAutomation, useDeleteAutomation, useUpdateAutomation } from './hooks';
import type { Automation } from './types';

export function AutomationsPage() {
  const { ws, isAdmin } = useWorkspace();
  const automations = useAutomations(ws);
  const labels = useLabels(ws, { includeArchived: true });
  const create = useCreateAutomation(ws);
  const update = useUpdateAutomation(ws);
  const remove = useDeleteAutomation(ws);
  const [editing, setEditing] = useState<Automation | 'new' | undefined>();
  const [runsFor, setRunsFor] = useState<Automation | undefined>();
  const [deleting, setDeleting] = useState<Automation | undefined>();

  const labelItems = labels.data?.items ?? [];
  const labelName = (key: string) => labelItems.find((l) => l.key === key)?.name ?? key;
  const items = automations.data?.items ?? [];
  const usedKeys = new Set(items.flatMap((a) => (a.preset_key ? [a.preset_key] : [])));
  const saving = create.isPending || update.isPending;

  const closeBuilder = () => {
    create.reset();
    update.reset();
    setEditing(undefined);
  };

  if (editing) {
    return (
      <Page>
        <PageHeader
          kicker="Automation"
          title={editing === 'new' ? 'New automation' : 'Edit automation'}
          description="Write it like a sentence: when an email looks like this, do these steps."
        />
        <AutomationBuilder
          key={editing === 'new' ? 'new' : editing.id}
          ws={ws}
          initial={editing === 'new' ? undefined : editing}
          labels={labelItems}
          saving={saving}
          error={create.error ?? update.error}
          onCancel={closeBuilder}
          onSave={(input) => {
            const opts = { onSuccess: closeBuilder };
            if (editing === 'new') create.mutate(input, opts);
            else update.mutate({ id: editing.id, ...input }, opts);
          }}
        />
      </Page>
    );
  }

  return (
    <Page>
      <PageHeader
        kicker="When X, do Y"
        title="Automations"
        description="Rules that run on every new email once the AI has read it, for example drafting a reply or reminding you when an important thread goes quiet."
        actions={isAdmin ? <Button onClick={() => setEditing('new')}>New automation</Button> : undefined}
      />
      {automations.isLoading && <Spinner />}
      <ErrorNote error={automations.error ?? update.error ?? remove.error} />
      {automations.data &&
        (items.length === 0 ? (
          <Card>
            <EmptyState
              title="No automations yet"
              description={isAdmin ? 'Pick a preset below or build your own.' : 'Ask a workspace admin to set one up.'}
            />
          </Card>
        ) : (
          <ul className="flex flex-col gap-4" aria-label="Automations">
            {items.map((a, i) => (
              <AutomationCard
                key={a.id}
                automation={a}
                index={i}
                isAdmin={isAdmin}
                togglePending={update.isPending && update.variables?.id === a.id}
                labelName={labelName}
                onToggle={(enabled) => update.mutate({ id: a.id, enabled })}
                onRuns={() => setRunsFor(a)}
                onEdit={() => setEditing(a)}
                onDelete={() => setDeleting(a)}
              />
            ))}
          </ul>
        ))}
      {isAdmin && automations.data && <PresetsGallery ws={ws} usedKeys={usedKeys} labelName={labelName} />}

      {runsFor && <RunsDrawer ws={ws} automation={runsFor} onClose={() => setRunsFor(undefined)} />}
      {deleting && (
        <Overlay title={`Delete “${deleting.name}”?`} titleId="delete-title" onClose={() => setDeleting(undefined)}>
          <p className="text-sm text-ink-muted">It stops running right away. Past runs are no longer listed.</p>
          <ErrorNote error={remove.error} />
          <div className="flex gap-2">
            <Button variant="secondary" onClick={() => setDeleting(undefined)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              loading={remove.isPending}
              onClick={() => remove.mutate(deleting.id, { onSuccess: () => setDeleting(undefined) })}
            >
              Delete
            </Button>
          </div>
        </Overlay>
      )}
    </Page>
  );
}
