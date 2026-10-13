import { useId, useState } from 'react';
import { Link } from 'react-router';
import { Badge, Button, Card, ConfirmDialog, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { useMessage } from '../inbox/hooks';
import { confidenceLevel, needsConfirmation, originOf } from './grouping';
import { useUpdateMemory } from './hooks';
import type { ConfidenceLevel, LearnedMemory } from './types';

const LEVELS: Record<ConfidenceLevel, { label: string; filled: number; bar: string }> = {
  sure: { label: 'Sure', filled: 3, bar: 'bg-success' },
  likely: { label: 'Likely', filled: 2, bar: 'bg-info' },
  unsure: { label: 'Unsure', filled: 1, bar: 'bg-review' },
};

export function ConfidenceMeter({ level }: { level: ConfidenceLevel }) {
  const { label, filled, bar } = LEVELS[level];
  return (
    <span className="inline-flex items-center gap-2 text-xs font-semibold text-ink">
      <span aria-hidden="true" className="flex gap-0.5">
        {[0, 1, 2].map((i) => (
          <span key={i} className={cn('h-2 w-4 rounded-xs', i < filled ? bar : 'bg-rule')} />
        ))}
      </span>
      <span>
        <span className="sr-only">Confidence: </span>
        {label}
      </span>
    </span>
  );
}

function SourceLink({ ws, messageId }: { ws: string; messageId: string }) {
  const message = useMessage(ws, messageId);
  const subject = message.data ? message.data.subject || '(no subject)' : 'the original email';
  return (
    <span className="min-w-0">
      Learned from:{' '}
      <Link to={`/w/${ws}/inbox/${messageId}`} className="rounded-xs text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover">
        {subject}
      </Link>
    </span>
  );
}

const usedText = (n: number) => (n === 0 ? 'Not used yet' : `Used ${n} ${n === 1 ? 'time' : 'times'}`);

export function LearnedItem({ ws, memory }: { ws: string; memory: LearnedMemory }) {
  const factId = useId();
  const update = useUpdateMemory(ws);
  const notify = useToast();
  const [deleting, setDeleting] = useState(false);
  const level = confidenceLevel(memory);
  const origin = originOf(memory);
  const busy = update.isPending;
  const confirm = () =>
    update.mutate(memory.status === 'suggested' ? { id: memory.id, confirmed: true, status: 'active' } : { id: memory.id, confirmed: true });

  return (
    <li>
      <Card className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          {memory.pinned && <Badge tone="info">Pinned</Badge>}
          {origin === 'manual' && <Badge>Added by you</Badge>}
          {memory.scope === 'contact' && memory.contact_email && (
            <Link
              to={`/w/${ws}/memory/contacts/${encodeURIComponent(memory.contact_email)}`}
              className="font-mono text-xs text-accent underline decoration-current/40 underline-offset-[3px] hover:text-accent-hover"
            >
              {memory.contact_email}
            </Link>
          )}
        </div>
        <p id={factId} className="text-[15px] break-words whitespace-pre-wrap">
          {memory.content}
        </p>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-ink-muted">
          <ConfidenceMeter level={level} />
          {origin === 'auto' && memory.source_message_id && <SourceLink ws={ws} messageId={memory.source_message_id} />}
          <span>{usedText(memory.use_count ?? 0)}</span>
        </div>
        {needsConfirmation(memory) && (
          <p className="text-sm text-ink-muted">The assistant is not sure about this one, so it will not use it until you confirm.</p>
        )}
        <div className="flex flex-wrap items-center gap-2">
          {needsConfirmation(memory) && (
            <Button size="sm" aria-describedby={factId} disabled={busy} onClick={confirm}>
              Confirm
            </Button>
          )}
          <Button
            size="sm"
            variant="secondary"
            aria-describedby={factId}
            disabled={busy}
            onClick={() => update.mutate({ id: memory.id, pinned: !memory.pinned })}
          >
            {memory.pinned ? 'Unpin' : 'Pin'}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            aria-describedby={factId}
            disabled={busy}
            onClick={() => setDeleting(true)}
          >
            Delete
          </Button>
        </div>
        <ErrorNote error={deleting ? undefined : update.error} />
        {deleting && (
          <ConfirmDialog
            title="Delete this fact?"
            confirmLabel="Delete"
            busy={busy}
            error={update.error}
            onCancel={() => {
              update.reset();
              setDeleting(false);
            }}
            onConfirm={() =>
              update.mutate(
                { id: memory.id, status: 'archived' },
                {
                  onSuccess: () => {
                    setDeleting(false);
                    notify('success', 'Fact deleted. The assistant will stop using it.');
                  },
                },
              )
            }
          >
            <p className="border-l-[3px] border-rule-strong pl-3 break-words text-ink">{memory.content}</p>
            <p className="mt-2">The assistant will stop using it when it labels and drafts.</p>
          </ConfirmDialog>
        )}
      </Card>
    </li>
  );
}
