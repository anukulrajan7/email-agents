import { useEffect, useRef, useState } from 'react';
import type { Draft, MessageDetail } from '../../api/types';
import { Button } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { StatusBadge } from '../../components/StatusBadges';
import { cn } from '../../lib/cn';
import { fullDate } from '../../lib/format';
import { extractNeeds, hasNeeds, isStillNeeded, splitNeeds } from './needs';
import { SendDialog } from './SendDialog';
import { useGenerateDraft, useSaveProviderDraft, useSendDraft, useUpdateDraft } from './hooks';

const POLL_LIMIT_MS = 60_000;

const replySubject = (subject: string) => (/^\s*re:/i.test(subject) ? subject.trim() : `Re: ${subject}`);

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
  const needsInput = hasNeeds(text);
  const checklist = Array.from(new Set(draft.unanswered ?? extractNeeds(draft.body_text)));

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
      {!sent && <NeedsChecklist items={checklist} text={text} />}
      <HighlightedTextarea text={text} onChange={setText} readOnly={sent} />
      {!sent && needsInput && (
        <p id="draft-needs-hint" className="text-sm text-review-ink">
          Fill every dashed [NEEDS: …] part to send. You can still save this to Gmail drafts.
        </p>
      )}
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
              <Button disabled={needsInput} aria-describedby={needsInput ? 'draft-needs-hint' : undefined} onClick={() => setConfirming(true)}>
                Send reply…
              </Button>
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
      <div className="flex items-center justify-between gap-2 border-b-[3px] border-double border-rule-strong pb-3">
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
