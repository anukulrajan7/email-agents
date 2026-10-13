import { useId, useState } from 'react';
import { Button, Label, Select } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { usePreviewAgent, useRecentEmails } from './hooks';
import type { AgentInput, UserProfileInput } from './types';

/** Writes a reply to a recent email with the current (unsaved) settings. Nothing is saved or sent. */
export function TestOnEmail({
  ws,
  agentName,
  agent,
  profile,
}: {
  ws: string;
  agentName: string;
  agent?: AgentInput;
  profile?: Partial<UserProfileInput>;
}) {
  const uid = useId();
  const [open, setOpen] = useState(false);
  const [messageId, setMessageId] = useState('');
  const emails = useRecentEmails(ws, open);
  const preview = usePreviewAgent(ws);
  const items = emails.data?.items ?? [];

  if (!open) {
    return (
      <Button variant="secondary" size="sm" className="self-start" onClick={() => setOpen(true)}>
        Test on an email
      </Button>
    );
  }

  return (
    <section aria-labelledby={`${uid}-title`} className="flex flex-col gap-3 rounded-sm border border-rule bg-surface p-4">
      <h3 id={`${uid}-title`} className="text-sm font-semibold text-ink">
        Test on an email
      </h3>
      <p className="text-sm text-ink-muted">Pick a recent email to see how {agentName} would reply. Nothing is saved or sent.</p>
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${uid}-email`}>Email</Label>
        <Select id={`${uid}-email`} value={messageId} onChange={(e) => setMessageId(e.target.value)} disabled={emails.isLoading}>
          <option value="">{emails.isLoading ? 'Loading recent emails…' : 'Choose an email'}</option>
          {items.map((m) => (
            <option key={m.id} value={m.id}>
              {m.subject || '(no subject)'} — {m.from}
            </option>
          ))}
        </Select>
      </div>
      <ErrorNote error={emails.error ?? preview.error} />
      <Button
        size="sm"
        className="self-start"
        disabled={!messageId}
        loading={preview.isPending}
        onClick={() => preview.mutate({ message_id: messageId, ...(agent ? { agent } : {}), ...(profile ? { profile } : {}) })}
      >
        Write a test reply
      </Button>
      {preview.data && (
        <div role="status" aria-label="Test reply" className="flex flex-col gap-1">
          <p className="kicker">{agentName} would write</p>
          {preview.data.needs_reply ? (
            <p className="border-l-[3px] border-rule-strong pl-3 text-sm whitespace-pre-wrap text-ink">{preview.data.body}</p>
          ) : (
            <p className="text-sm text-ink-muted">{agentName} thinks this email doesn’t need a reply.</p>
          )}
        </div>
      )}
    </section>
  );
}
