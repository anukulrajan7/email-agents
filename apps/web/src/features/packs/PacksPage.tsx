import { useState } from 'react';
import { ErrorNote } from '../../components/ErrorNote';
import { Button, Card, EmptyState, Page, PageHeader, Skeleton, useToast } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { useWorkspace } from '../../app/useWorkspace';
import { HelpPopover } from '../help/HelpPopover';
import { PackPicker } from './PackPicker';
import { usePackPreview, useSetPacks, useWorkspacePacks } from './hooks';

export function PacksPage() {
  const { ws, isAdmin } = useWorkspace();
  const packs = useWorkspacePacks(ws);
  const save = useSetPacks(ws);
  const notify = useToast();
  const [draft, setDraft] = useState<string[] | undefined>(undefined);
  const enabled = packs.data?.enabled ?? [];
  const selected = draft ?? enabled;
  const changed = draft !== undefined && (draft.length !== enabled.length || draft.some((k) => !enabled.includes(k)));

  const canSave = changed && selected.length > 0;
  const preview = usePackPreview(ws, selected, isAdmin && canSave);
  const added = (preview.data?.labels_added.length ?? 0) + (preview.data?.labels_restored.length ?? 0);
  const hidden = preview.data?.labels_hidden.length ?? 0;

  if (!isAdmin) {
    return (
      <Page>
        <PageHeader kicker="Settings" title="Packs" />
        <EmptyState title="Admins only" description="Ask a workspace admin to change which packs are on." />
      </Page>
    );
  }

  return (
    <Page>
      <PageHeader
        kicker="Settings"
        title="Packs"
        actions={<HelpPopover topic="packs" />}
        description="A pack is a bundle for one kind of work: labels, automation presets, reply style and features. Turning a pack on adds its labels. Turning it off hides its labels and stops its features; your mail and data stay."
      />
      {packs.isLoading && <Skeleton className="h-40" />}
      <ErrorNote error={packs.error} />
      {packs.data && (
        <Card className="rise flex flex-col gap-5 p-6" style={riseStyle(0)}>
          <PackPicker packs={packs.data.items} selected={selected} onChange={setDraft} disabled={save.isPending} />
          {canSave && preview.data && (
            <p className="text-sm text-ink-muted" role="status">
              {added + hidden === 0
                ? 'No label changes.'
                : `This will add ${added} and hide ${hidden} ${added === 1 && hidden === 0 ? 'label' : 'labels'}. Hidden labels keep their mail and come back when you turn the pack on again.`}
            </p>
          )}
          {canSave && <ErrorNote error={preview.error} />}
          <ErrorNote error={save.error} />
          <div>
            <Button
              loading={save.isPending}
              disabled={!canSave || preview.isError}
              onClick={() =>
                save.mutate(selected, {
                  onSuccess: () => {
                    setDraft(undefined);
                    notify('success', 'Packs updated.');
                  },
                })
              }
            >
              Save packs
            </Button>
          </div>
        </Card>
      )}
    </Page>
  );
}
