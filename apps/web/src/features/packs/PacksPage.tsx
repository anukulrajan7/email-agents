import { useState } from 'react';
import { ErrorNote } from '../../components/ErrorNote';
import { Button, Card, EmptyState, Page, PageHeader, Skeleton, useToast } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { useWorkspace } from '../../app/useWorkspace';
import { HelpPopover } from '../help/HelpPopover';
import { PackPicker } from './PackPicker';
import { useSetPacks, useWorkspacePacks } from './hooks';

export function PacksPage() {
  const { ws, isAdmin } = useWorkspace();
  const packs = useWorkspacePacks(ws);
  const save = useSetPacks(ws);
  const notify = useToast();
  const [draft, setDraft] = useState<string[] | undefined>(undefined);
  const enabled = packs.data?.enabled ?? [];
  const selected = draft ?? enabled;
  const changed = draft !== undefined && (draft.length !== enabled.length || draft.some((k) => !enabled.includes(k)));

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
        description="A pack is a bundle for one kind of work: labels, automation presets, reply style and features. Turning a pack on adds its labels. Turning it off keeps your data and only stops its features."
      />
      {packs.isLoading && <Skeleton className="h-40" />}
      <ErrorNote error={packs.error} />
      {packs.data && (
        <Card className="rise flex flex-col gap-5 p-6" style={riseStyle(0)}>
          <PackPicker packs={packs.data.items} selected={selected} onChange={setDraft} disabled={save.isPending} />
          <ErrorNote error={save.error} />
          <div>
            <Button
              loading={save.isPending}
              disabled={!changed || selected.length === 0}
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
