import { useState } from 'react';
import { Button, Card, ConfirmDialog, EmptyState, Page, PageHeader, SkeletonList, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { useLabels } from '../labels/hooks';
import { AutomationBuilder } from './Builder';
import { AutomationCard } from './AutomationCard';
import { PresetsGallery } from './PresetsGallery';
import { ReplySettingsCard } from './ReplySettingsCard';
import { RunsDrawer } from './RunsDrawer';
import { useAutomations, useCreateAutomation, useDeleteAutomation, useUpdateAutomation } from './hooks';
import type { Automation } from './types';

export function AutomationsPage() {
  const { ws, isAdmin, hasStaffing } = useWorkspace();
  const automations = useAutomations(ws);
  const labels = useLabels(ws, { includeArchived: true });
  const create = useCreateAutomation(ws);
  const update = useUpdateAutomation(ws);
  const remove = useDeleteAutomation(ws);
  const notify = useToast();
  const [editing, setEditing] = useState<Automation | 'new' | undefined>();
  const [runsFor, setRunsFor] = useState<Automation | undefined>();
  const [deleting, setDeleting] = useState<Automation | undefined>();

  const labelItems = labels.data?.items ?? [];
  const labelName = (key: string) => labelItems.find((l) => l.key === key)?.name ?? key;
  const items = automations.data?.items ?? [];
  const usedKeys = new Set(items.flatMap((a) => (a.preset_key ? [a.preset_key] : [])));
  const saving = create.isPending || update.isPending;
  const empty = automations.data !== undefined && items.length === 0;

  const closeBuilder = () => {
    create.reset();
    update.reset();
    setEditing(undefined);
  };

  if (editing) {
    return (
      <Page>
        <button
          type="button"
          onClick={closeBuilder}
          className="-mb-2 self-start rounded-xs text-sm text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover"
        >
          ← All automations
        </button>
        <PageHeader
          kicker="Automation"
          title={editing === 'new' ? 'New automation' : 'Edit automation'}
          description="Write it like a sentence: when an email looks like this, do these steps. The preview on the right reads it back to you."
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
            const done = (message: string) => () => {
              closeBuilder();
              notify('success', message);
            };
            if (editing === 'new') create.mutate(input, { onSuccess: done(`“${input.name}” created.`) });
            else update.mutate({ id: editing.id, ...input }, { onSuccess: done('Changes saved.') });
          }}
        />
      </Page>
    );
  }

  const gallery = isAdmin && automations.data && (
    <PresetsGallery ws={ws} usedKeys={usedKeys} labelName={labelName} onAdded={(name) => notify('success', `“${name}” added.`)} />
  );

  return (
    <Page>
      <PageHeader
        guide="automations-header"
        kicker="When X, do Y"
        title="Automations"
        description="Rules that run on every new email once it has been read, for example drafting a reply or reminding you when an important thread goes quiet."
        actions={isAdmin ? <Button onClick={() => setEditing('new')}>New automation</Button> : undefined}
      />
      <ReplySettingsCard ws={ws} isAdmin={isAdmin} hasStaffing={hasStaffing} />
      {automations.isLoading && <SkeletonList rows={2} label="Loading automations" />}
      <ErrorNote error={automations.error ?? update.error} />
      {empty && gallery}
      {empty && (
        <Card>
          <EmptyState
            title={isAdmin ? 'Or build your own' : 'No automations yet'}
            description={isAdmin ? 'Start from a blank sentence and add the conditions and steps you need.' : 'Ask a workspace admin to set one up.'}
            action={isAdmin ? <Button variant="secondary" onClick={() => setEditing('new')}>Build an automation</Button> : undefined}
          />
        </Card>
      )}
      {items.length > 0 && (
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
      )}
      {!empty && gallery}

      {runsFor && <RunsDrawer ws={ws} automation={runsFor} onClose={() => setRunsFor(undefined)} />}
      {deleting && (
        <ConfirmDialog
          title={`Delete “${deleting.name}”?`}
          confirmLabel="Delete"
          busy={remove.isPending}
          error={remove.error}
          onCancel={() => {
            remove.reset();
            setDeleting(undefined);
          }}
          onConfirm={() =>
            remove.mutate(deleting.id, {
              onSuccess: () => {
                notify('success', `“${deleting.name}” deleted.`);
                setDeleting(undefined);
              },
            })
          }
        >
          It stops running right away. Past runs are no longer listed.
        </ConfirmDialog>
      )}
    </Page>
  );
}
