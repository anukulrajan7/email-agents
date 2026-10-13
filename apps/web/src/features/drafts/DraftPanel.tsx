import { useEffect, useState } from 'react';
import type { Draft, MessageDetail } from '../../api/types';
import { Button, Textarea } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { useGenerateDraft, useUpdateDraft } from './hooks';

const POLL_LIMIT_MS = 60_000;

function DraftEditor({ ws, messageId, draft }: { ws: string; messageId: string; draft: Draft }) {
  const [text, setText] = useState(draft.body_text);
  const [copied, setCopied] = useState(false);
  const update = useUpdateDraft(ws, messageId, draft.id);

  const copy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className="flex flex-col gap-3">
      <label htmlFor="draft-body" className="sr-only">
        Draft reply
      </label>
      <Textarea id="draft-body" rows={10} className="ruled bg-surface px-4 py-0 text-[15px]" value={text} onChange={(e) => setText(e.target.value)} />
      <div className="flex flex-wrap gap-2">
        <Button
          variant="secondary"
          loading={update.isPending && update.variables?.body_text !== undefined}
          disabled={text === draft.body_text}
          onClick={() => update.mutate({ body_text: text, status: 'edited' })}
        >
          Save
        </Button>
        <Button
          loading={update.isPending && update.variables?.body_text === undefined}
          disabled={draft.status === 'approved' && text === draft.body_text}
          onClick={() => update.mutate(text === draft.body_text ? { status: 'approved' } : { body_text: text, status: 'approved' })}
        >
          Approve
        </Button>
        <Button variant="secondary" onClick={copy}>
          {copied ? 'Copied' : 'Copy draft'}
        </Button>
      </div>
      <ErrorNote error={update.error} />
    </div>
  );
}

export function DraftPanel({
  ws,
  message,
  onAwaitingChange,
}: {
  ws: string;
  message: MessageDetail;
  onAwaitingChange: (awaiting: boolean) => void;
}) {
  const generate = useGenerateDraft(ws, message.id);
  const [baseline, setBaseline] = useState<{ updatedAt: string | null } | undefined>();
  const draft = message.draft;
  const awaiting = baseline !== undefined && (draft?.updated_at ?? null) === baseline.updatedAt;

  useEffect(() => {
    onAwaitingChange(awaiting);
    if (!awaiting) return;
    const id = setTimeout(() => setBaseline(undefined), POLL_LIMIT_MS);
    return () => clearTimeout(id);
  }, [awaiting, onAwaitingChange]);

  const start = () =>
    generate.mutate(undefined, { onSuccess: () => setBaseline({ updatedAt: draft?.updated_at ?? null }) });

  return (
    <section aria-labelledby="draft-heading" className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-2 border-b-[3px] border-double border-rule-strong pb-3">
        <h3 id="draft-heading" className="type-display flex items-center gap-3 text-2xl tracking-[-0.01em]">
          Draft reply {draft && <StatusBadge status={draft.status} />}
        </h3>
        <Button size="sm" variant="secondary" loading={generate.isPending || awaiting} onClick={start}>
          {draft ? 'Regenerate' : 'Generate draft'}
        </Button>
      </div>
      <p className="min-h-5 text-sm text-ink-muted" aria-live="polite">
        {awaiting ? 'Generating draft…' : !draft ? 'No draft yet.' : ''}
      </p>
      <ErrorNote error={generate.error} />
      {message.source.kind === 'forwarding' && (
        <p className="-rotate-[0.4deg] border-b border-dashed border-review border-l-[3px] bg-review-tint px-3 py-2 text-sm text-review-ink">
          Forwarded emails can't create drafts in the original mailbox — copy this text into your mail client.
        </p>
      )}
      {draft ? (
        <DraftEditor key={`${draft.id}:${draft.updated_at}`} ws={ws} messageId={message.id} draft={draft} />
      ) : null}
      {message.source.can_create_provider_draft && draft && (
        <Button variant="secondary" className="self-start" disabled title="Provider drafts are not wired up yet">
          Create draft in mailbox
        </Button>
      )}
    </section>
  );
}
