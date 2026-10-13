import { useState } from 'react';
import { Button, Card, CardTitle, SkeletonList, Switch } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { riseStyle } from '../../lib/motion';
import { useMe } from '../auth/hooks';
import { SEND_WARNING } from '../automations/ActionRow';
import { AutomationSentence } from '../automations/Sentence';
import { useAutomations, useCreateFromPreset, usePresets, useUpdateAutomation } from '../automations/hooks';
import { SuggestLabels } from '../labels/SuggestLabels';
import { useLabels } from '../labels/hooks';
import { isReplySetupDone, markReplySetupDone } from './storage';

function RepliesStep({ ws, onNext, onSkip }: { ws: string; onNext: () => void; onSkip: () => void }) {
  const presets = usePresets(ws);
  const automations = useAutomations(ws);
  const labels = useLabels(ws, { includeArchived: true });
  const create = useCreateFromPreset(ws);
  const update = useUpdateAutomation(ws);
  const labelName = (key: string) => labels.data?.items.find((l) => l.key === key)?.name ?? key;
  const busyKey = create.isPending ? create.variables : update.isPending ? update.variables?.id : undefined;

  return (
    <>
      <p className="text-sm text-ink-muted">
        Pick what happens when mail arrives. You can change any of this later under Automations.
      </p>
      {(presets.isLoading || automations.isLoading) && <SkeletonList rows={2} label="Loading reply options" />}
      <ErrorNote error={presets.error ?? automations.error ?? create.error ?? update.error} />
      <ul className="flex flex-col divide-y divide-rule" aria-label="Reply options">
        {presets.data?.items.map((p) => {
          const existing = automations.data?.items.find((a) => a.preset_key === p.key);
          const sends = p.automation.actions.some((a) => a.type === 'send_reply');
          const busy = busyKey === p.key || (existing !== undefined && busyKey === existing.id);
          return (
            <li key={p.key} className="flex items-start gap-3 py-3">
              <Switch
                checked={existing?.enabled ?? false}
                label={p.name}
                disabled={busy || !automations.data}
                onChange={(enabled) => (existing ? update.mutate({ id: existing.id, enabled }) : create.mutate(p.key))}
              />
              <div className="min-w-0 flex-1">
                <p className="text-sm font-semibold text-ink">{p.name}</p>
                <p className="text-sm text-ink-muted">{p.description}</p>
                <AutomationSentence
                  className="mt-1 text-sm leading-6 text-ink"
                  conditions={p.automation.conditions}
                  actions={p.automation.actions}
                  labelName={labelName}
                />
                {sends && <p className="mt-1 text-sm text-review-ink">{SEND_WARNING}</p>}
              </div>
            </li>
          );
        })}
      </ul>
      <div className="flex gap-2">
        <Button onClick={onNext}>Next</Button>
        <Button variant="ghost" onClick={onSkip}>
          Skip for now
        </Button>
      </div>
    </>
  );
}

export function ReplySetup({ ws }: { ws: string }) {
  const me = useMe();
  const [hidden, setHidden] = useState(() => isReplySetupDone(ws));
  const [step, setStep] = useState<'replies' | 'labels'>('replies');
  const isAdmin = me.data?.workspaces.find((w) => w.id === ws)?.role === 'admin';

  if (hidden || !isAdmin) return null;

  const finish = () => {
    markReplySetupDone(ws);
    setHidden(true);
  };

  return (
    <Card className="rise m-4 flex max-w-2xl flex-col gap-3 bg-raised p-5 sm:m-6 sm:p-6" style={riseStyle(0)} role="region" aria-labelledby="reply-setup-heading">
      <div className="flex items-start justify-between gap-4">
        <div>
          <p className="kicker">{step === 'replies' ? 'Step 1 of 2' : 'Step 2 of 2'}</p>
          <CardTitle id="reply-setup-heading">{step === 'replies' ? 'How should we handle replies?' : 'Sort your mail into labels'}</CardTitle>
        </div>
      </div>
      {step === 'replies' ? (
        <RepliesStep ws={ws} onNext={() => setStep('labels')} onSkip={finish} />
      ) : (
        <>
          <p className="text-sm text-ink-muted">We can read your recent mail and propose labels that fit how your team works.</p>
          <SuggestLabels ws={ws} />
          <div className="flex gap-2">
            <Button onClick={finish}>Done</Button>
            <Button variant="ghost" onClick={finish}>
              Skip for now
            </Button>
          </div>
        </>
      )}
    </Card>
  );
}
