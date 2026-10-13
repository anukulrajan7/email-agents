import { useEffect, useState } from 'react';
import type { Draft, MessageDetail } from '../../api/types';
import { Button, Textarea } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { fullDate } from '../../lib/format';
import { SendDialog } from './SendDialog';
import { useGenerateDraft, useSaveProviderDraft, useSendDraft, useUpdateDraft } from './hooks';

const POLL_LIMIT_MS = 60_000;

const replySubject = (subject: string) => (/^\s*re:/i.test(subject) ? subject.trim() : `Re: ${subject}`);

function DraftEditor({
  ws,
  message,
  draft,
  savedToGmail,
  onSavedToGmail,
}: {
  ws: string;
  message: MessageDetail;
  draft: Draft;
  savedToGmail: boolean;
  onSavedToGmail: () => void;
}) {
  const messageId = message.id;
  const [text, setText] = useState(draft.body_text);
  const [copied, setCopied] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const update = useUpdateDraft(ws, messageId, draft.id);
  const saveProvider = useSaveProviderDraft(ws, messageId, draft.id);
  const send = useSendDraft(ws, messageId, draft.id);
  const sent = draft.status === 'sent';
  const dirty = text !== draft.body_text;

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
      <Textarea
        id="draft-body"
        rows={10}
        readOnly={sent}
        className="ruled bg-surface px-4 py-0 text-[15px]"
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      {sent && (
        <p className="text-sm font-semibold text-ink-muted" aria-live="polite">
          Sent ✓ — Sent from Gmail on {fullDate(draft.updated_at)}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        {!sent && (
          <>
            <Button
              variant="secondary"
              loading={update.isPending && update.variables?.body_text !== undefined}
              disabled={!dirty}
              onClick={() => update.mutate({ body_text: text, status: 'edited' })}
            >
              Save
            </Button>
            <Button
              loading={update.isPending && update.variables?.body_text === undefined}
              disabled={draft.status === 'approved' && !dirty}
              onClick={() => update.mutate(!dirty ? { status: 'approved' } : { body_text: text, status: 'approved' })}
            >
              Approve
            </Button>
            {message.source.can_create_provider_draft && (
              <Button
                variant="secondary"
                loading={saveProvider.isPending}
                onClick={() => saveProvider.mutate(dirty ? text : undefined, { onSuccess: onSavedToGmail })}
              >
                Save to Gmail drafts
              </Button>
            )}
            {message.source.can_send && (
              <Button onClick={() => setConfirming(true)}>Send reply…</Button>
            )}
          </>
        )}
        <Button variant="secondary" onClick={copy}>
          {copied ? 'Copied' : 'Copy draft'}
        </Button>
      </div>
      {savedToGmail && !sent && (
        <p className="text-sm text-ink-muted" role="status">
          Saved in your Gmail Drafts — you can finish it there too.
        </p>
      )}
      <ErrorNote error={update.error ?? saveProvider.error} />
      {confirming && (
        <SendDialog
          to={message.from}
          subject={replySubject(message.subject)}
          body={text}
          sending={send.isPending}
          error={send.error}
          onCancel={() => {
            send.reset();
            setConfirming(false);
          }}
          onConfirm={() => send.mutate(dirty ? text : undefined, { onSuccess: () => setConfirming(false) })}
        />
      )}
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
  const [savedToGmail, setSavedToGmail] = useState(false);
  const draft = message.draft;
  const awaiting = baseline !== undefined && (draft?.updated_at ?? null) === baseline.updatedAt;

  useEffect(() => {
    onAwaitingChange(awaiting);
    if (!awaiting) return;
    const id = setTimeout(() => setBaseline(undefined), POLL_LIMIT_MS);
    return () => clearTimeout(id);
  }, [awaiting, onAwaitingChange]);

  const start = () =>
    generate.mutate(undefined, {
      onSuccess: () => {
        setSavedToGmail(false);
        setBaseline({ updatedAt: draft?.updated_at ?? null });
      },
    });

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
        <DraftEditor
          key={`${draft.id}:${draft.updated_at}`}
          ws={ws}
          message={message}
          draft={draft}
          savedToGmail={savedToGmail}
          onSavedToGmail={() => setSavedToGmail(true)}
        />
      ) : null}
      {draft && draft.memories_used.length > 0 && (
        <details className="text-sm">
          <summary className="cursor-pointer font-semibold text-ink-muted hover:text-ink focus-visible:outline-2 focus-visible:outline-accent">
            Used {draft.memories_used.length} {draft.memories_used.length === 1 ? 'memory' : 'memories'}
          </summary>
          <ul className="mt-2 flex flex-col gap-1 border-l-[3px] border-rule-strong pl-3">
            {draft.memories_used.map((u) => (
              <li key={u.id} className="whitespace-pre-wrap break-words">
                {u.content}
              </li>
            ))}
          </ul>
        </details>
      )}
    </section>
  );
}
