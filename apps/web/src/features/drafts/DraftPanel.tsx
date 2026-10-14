import { useEffect, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import type { Draft, MessageDetail } from '../../api/types';
import { Button, MoreMenu, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { cn } from '../../lib/cn';
import { fullDate } from '../../lib/format';
import { extractNeeds, hasNeeds, isStillNeeded, splitNeeds } from './needs';
import { useGenerateDraft, useSaveProviderDraft, useSendDraft, useUndoSend, useUpdateDraft } from './hooks';
import { messageKey, messagesKey } from '../inbox/hooks';

const POLL_LIMIT_MS = 60_000;

function NeedsChecklist({ items, text }: { items: string[]; text: string }) {
  if (items.length === 0) return null;
  return (
    <section aria-label="Fill these before sending" className="flex flex-col gap-2 border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
      <h4 className="font-semibold">Fill these before sending</h4>
      <ul className="flex flex-col gap-1">
        {items.map((item) => {
          const pending = isStillNeeded(text, item);
          return (
            <li key={item} className="flex items-start gap-2">
              <span aria-hidden="true" className="mt-px font-mono">
                {pending ? '[ ]' : '[x]'}
              </span>
              <span className={cn('break-words', !pending && 'line-through')}>
                {item}
                <span className="sr-only">{pending ? ' (still needed)' : ' (done)'}</span>
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

function UnconfirmedNote({ items }: { items: string[] }) {
  if (items.length === 0) return null;
  return (
    <p role="note" className="border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
      Couldn't confirm: {items.join(', ')}
    </p>
  );
}

function HighlightedTextarea({ text, onChange, readOnly }: { text: string; onChange: (value: string) => void; readOnly: boolean }) {
  const ref = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = 'auto';
    if (el.scrollHeight > 0) el.style.height = `${el.scrollHeight}px`;
  }, [text]);

  const parts = splitNeeds(text);
  return (
    <div className="relative border border-rule-strong bg-surface focus-within:border-ink focus-within:outline-2 focus-within:outline-offset-2 focus-within:outline-accent">
      <div
        aria-hidden="true"
        data-testid="draft-highlights"
        className="pointer-events-none absolute inset-0 overflow-hidden select-none px-4 py-0 text-[15px] leading-[28px] break-words whitespace-pre-wrap text-transparent"
      >
        {parts.map((p, i) =>
          p.needs ? (
            <mark key={i} className="bg-review-tint text-transparent underline decoration-review-ink decoration-2 underline-offset-4 [text-decoration-style:dashed]">
              {p.text}
            </mark>
          ) : (
            <span key={i}>{p.text}</span>
          ),
        )}
        {'\n'}
      </div>
      <textarea
        ref={ref}
        id="draft-body"
        rows={10}
        readOnly={readOnly}
        value={text}
        onChange={(e) => onChange(e.target.value)}
        className="ruled relative block w-full resize-none overflow-hidden bg-transparent px-4 py-0 text-[15px] text-ink focus:outline-none"
      />
    </div>
  );
}

const secondsLeft = (sendAt: string | undefined, now: number) => {
  const at = sendAt ? Date.parse(sendAt) : Number.NaN;
  return Number.isNaN(at) ? 0 : Math.max(0, Math.ceil((at - now) / 1000));
};

/** "Sending in 30 s · Undo" while a send is scheduled; refreshes the email once the time is up. */
export function UndoBar({
  ws,
  messageId,
  draftId,
  sendAt,
  onUndone,
}: {
  ws: string;
  messageId: string;
  draftId: string;
  sendAt: string | undefined;
  onUndone: () => void;
}) {
  const qc = useQueryClient();
  const undo = useUndoSend(ws, messageId, draftId);
  const [now, setNow] = useState(() => Date.now());
  const left = secondsLeft(sendAt, now);

  useEffect(() => {
    if (left === 0) return;
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [left]);

  useEffect(() => {
    if (left > 0) return;
    const refresh = () =>
      Promise.all([
        qc.invalidateQueries({ queryKey: messageKey(ws, messageId) }),
        qc.invalidateQueries({ queryKey: messagesKey(ws) }),
      ]);
    void refresh();
    const id = setTimeout(refresh, 3000);
    return () => clearTimeout(id);
  }, [left, qc, ws, messageId]);

  return (
    <div role="status" className="flex flex-col gap-2 border-l-[3px] border-info bg-info-tint px-3 py-2 text-sm text-info">
      <div className="flex items-center justify-between gap-3">
        <span aria-live="polite">{left > 0 ? `Sending in ${left} s` : 'Sending…'}</span>
        {left > 0 && (
          <Button size="sm" variant="secondary" loading={undo.isPending} onClick={() => undo.mutate(undefined, { onSuccess: onUndone })}>
            Undo
          </Button>
        )}
      </div>
      <ErrorNote error={undo.error} />
    </div>
  );
}

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
  const [editing, setEditing] = useState(false);
  const update = useUpdateDraft(ws, messageId, draft.id);
  const saveProvider = useSaveProviderDraft(ws, messageId, draft.id);
  const send = useSendDraft(ws, messageId, draft.id);
  const notify = useToast();
  const sent = draft.status === 'sent';
  const sendAt = draft.status === 'scheduled' ? (draft.send_at ?? undefined) : send.data?.send_at;
  const scheduled = !sent && (draft.status === 'scheduled' || send.data !== undefined);
  const locked = sent || scheduled;
  const readOnly = locked || !editing;
  const dirty = text !== draft.body_text;
  const needsInput = hasNeeds(text);
  const checklist = Array.from(new Set(draft.unanswered ?? extractNeeds(draft.body_text)));

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      notify('error', 'Could not copy. Select the text and copy it yourself.');
    }
  };

  return (
    <div className="flex flex-col gap-3">
      <label htmlFor="draft-body" className="sr-only">
        Draft reply
      </label>
      {!locked && (hasNeeds(draft.body_text) ? <NeedsChecklist items={checklist} text={text} /> : <UnconfirmedNote items={draft.unanswered ?? []} />)}
      <HighlightedTextarea text={text} onChange={setText} readOnly={readOnly} />
      {scheduled && <UndoBar ws={ws} messageId={messageId} draftId={draft.id} sendAt={sendAt} onUndone={() => send.reset()} />}
      {!locked && draft.send_error && (
        <p role="alert" className="border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
          {draft.send_error}
        </p>
      )}
      {!locked && needsInput && (
        <p id="draft-needs-hint" className="text-sm text-review-ink">
          Fill every dashed [NEEDS: …] part to send. You can still save this to Gmail drafts.
        </p>
      )}
      {sent && (
        <p className="text-sm font-semibold text-ink-muted" aria-live="polite">
          Sent ✓ — Sent from Gmail on {fullDate(draft.updated_at)}
        </p>
      )}
      {!locked && (
        <div className="flex flex-wrap items-center gap-2">
          {message.source.can_send && (
            <Button
              disabled={needsInput}
              aria-describedby={needsInput ? 'draft-needs-hint' : undefined}
              loading={send.isPending}
              onClick={() => send.mutate(dirty ? text : undefined, { onSuccess: () => setEditing(false) })}
            >
              Send
            </Button>
          )}
          {editing && dirty ? (
            <Button
              variant="secondary"
              loading={update.isPending}
              onClick={() =>
                update.mutate(
                  { body_text: text, status: 'edited' },
                  {
                    onSuccess: () => {
                      setEditing(false);
                      notify('success', 'Draft saved.');
                    },
                  },
                )
              }
            >
              Save
            </Button>
          ) : (
            <Button variant="secondary" onClick={() => setEditing(true)} disabled={editing}>
              Edit
            </Button>
          )}
          <MoreMenu
            items={[
              ...(message.source.can_create_provider_draft
                ? [
                    {
                      label: 'Save to Gmail drafts',
                      loading: saveProvider.isPending,
                      onSelect: () => saveProvider.mutate(dirty ? text : undefined, { onSuccess: onSavedToGmail }),
                    },
                  ]
                : []),
              { label: copied ? 'Copied' : 'Copy draft', onSelect: copy },
              {
                label: 'Discard my edits',
                disabled: !dirty,
                onSelect: () => {
                  setText(draft.body_text);
                  setEditing(false);
                },
              },
            ]}
          />
        </div>
      )}
      {savedToGmail && !locked && (
        <p className="text-sm text-ink-muted" role="status">
          Saved in your Gmail Drafts — you can finish it there too.
        </p>
      )}
      <ErrorNote error={update.error ?? saveProvider.error ?? send.error} />
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

  const start = (better = false) =>
    generate.mutate(better ? { quality: 'premium' } : undefined, {
      onSuccess: () => {
        setSavedToGmail(false);
        setBaseline({ updatedAt: draft?.updated_at ?? null });
      },
    });
  const busy = generate.isPending || awaiting;

  return (
    <section aria-labelledby="draft-heading" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2 border-b-[3px] border-double border-rule-strong pb-3">
        <h3 id="draft-heading" className="type-display flex items-center gap-3 text-2xl tracking-[-0.01em]">
          Draft reply {draft && <StatusBadge status={draft.status} />}
        </h3>
        <div className="flex flex-wrap justify-end gap-2">
          {draft && (
            <Button size="sm" variant="secondary" disabled={busy} onClick={() => start(true)}>
              Write a better draft
            </Button>
          )}
          <Button size="sm" variant="secondary" loading={busy} onClick={() => start()}>
            {draft ? 'Regenerate' : 'Generate draft'}
          </Button>
        </div>
      </div>
      <p className="min-h-5 text-sm text-ink-muted" aria-live="polite">
        {awaiting ? 'Writing a draft…' : !draft ? 'No draft yet. Generate one when you are ready to reply.' : ''}
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
