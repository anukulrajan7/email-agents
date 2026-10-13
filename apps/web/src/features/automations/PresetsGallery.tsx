import { Badge, Button, Card, CardTitle, SkeletonList } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { riseStyle } from '../../lib/motion';
import { AutomationSentence } from './Sentence';
import { useCreateFromPreset, usePresets } from './hooks';

export function PresetsGallery({
  ws,
  usedKeys,
  labelName,
  heading = 'Start from a preset',
  onAdded,
}: {
  ws: string;
  usedKeys: Set<string>;
  labelName: (key: string) => string;
  heading?: string;
  onAdded?: (name: string) => void;
}) {
  const presets = usePresets(ws);
  const create = useCreateFromPreset(ws);

  return (
    <section aria-labelledby="presets-heading" className="flex flex-col gap-4">
      <h2 id="presets-heading" className="type-display text-2xl tracking-[-0.01em]">
        {heading}
      </h2>
      {presets.isLoading && <SkeletonList rows={2} label="Loading presets" />}
      <ErrorNote error={presets.error ?? create.error} />
      <div className="grid auto-rows-fr gap-4 md:grid-cols-2 2xl:grid-cols-3">
        {presets.data?.items.map((p, i) => {
          const used = usedKeys.has(p.key);
          return (
            <Card key={p.key} className="rise flex h-full flex-col gap-3" style={riseStyle(i + 1)}>
              <div className="flex items-start justify-between gap-2">
                <CardTitle>{p.name}</CardTitle>
                {used && <Badge tone="success">Added</Badge>}
              </div>
              <p className="text-sm text-ink-muted">{p.description}</p>
              <AutomationSentence
                className="text-sm leading-6 text-ink"
                conditions={p.automation.conditions}
                actions={p.automation.actions}
                labelName={labelName}
              />
              <div className="mt-auto border-t border-rule pt-3">
                <Button
                  size="sm"
                  variant={used ? 'secondary' : 'primary'}
                  aria-label={used ? undefined : `Use this preset: ${p.name}`}
                  disabled={used}
                  loading={create.isPending && create.variables === p.key}
                  onClick={() => create.mutate(p.key, { onSuccess: () => onAdded?.(p.name) })}
                >
                  {used ? 'Added' : 'Use this'}
                </Button>
              </div>
            </Card>
          );
        })}
      </div>
    </section>
  );
}
