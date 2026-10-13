import { useState } from 'react';
import { Button, Card, CardTitle, Input, Label as FieldLabel, Textarea } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { Switch } from './Switch';
import { useReplySettings, useSaveReplySettings } from './hooks';
import type { ReplySettings } from './types';

const MAX_INSTRUCTIONS = 2000;
const MAX_SIGNATURE = 200;

function ReplySettingsForm({ ws, initial, isAdmin }: { ws: string; initial: ReplySettings; isAdmin: boolean }) {
  const save = useSaveReplySettings(ws);
  const [autoReply, setAutoReply] = useState(initial.auto_reply ?? false);
  const [instructions, setInstructions] = useState(initial.reply_instructions ?? '');
  const [signature, setSignature] = useState(initial.signature ?? '');
  const dirty =
    autoReply !== (initial.auto_reply ?? false) ||
    instructions !== (initial.reply_instructions ?? '') ||
    signature !== (initial.signature ?? '');
  const defaultStyle = initial.default_reply_style;

  return (
    <form
      className="flex flex-col gap-4"
      onSubmit={(e) => {
        e.preventDefault();
        save.mutate({ auto_reply: autoReply, reply_instructions: instructions.trim(), signature: signature.trim() || null });
      }}
    >
      <div className="flex items-start justify-between gap-4">
        <div className="flex flex-col gap-1">
          <span id="auto-reply-label" className="text-sm font-semibold text-ink">
            Draft replies automatically for new emails that need a reply
          </span>
          <p className="text-sm text-ink-muted">Off: drafts are written only when you click Generate draft on an email.</p>
        </div>
        <Switch checked={autoReply} label="Draft replies automatically" disabled={!isAdmin} onChange={setAutoReply} />
      </div>

      <div className="flex flex-col gap-1">
        <FieldLabel htmlFor="reply-instructions">How should replies sound?</FieldLabel>
        <Textarea
          id="reply-instructions"
          rows={4}
          maxLength={MAX_INSTRUCTIONS}
          readOnly={!isAdmin}
          placeholder="Example: Warm and short. Always mention we reply within 1 working day. Never discuss salary."
          value={instructions}
          onChange={(e) => setInstructions(e.target.value)}
        />
        <p className="text-sm text-ink-muted">Leave this empty to use our standard guidance for an HR and staffing team.</p>
        {defaultStyle && (
          <details className="text-sm">
            <summary className="cursor-pointer font-semibold text-ink-muted hover:text-ink focus-visible:outline-2 focus-visible:outline-accent">
              See the standard guidance
            </summary>
            <p className="mt-2 border-l-[3px] border-rule-strong pl-3 whitespace-pre-wrap text-ink-muted">{defaultStyle}</p>
            {isAdmin && (
              <Button className="mt-2" size="sm" variant="secondary" onClick={() => setInstructions(defaultStyle)}>
                Use as a starting point
              </Button>
            )}
          </details>
        )}
      </div>

      <div className="flex flex-col gap-1">
        <FieldLabel htmlFor="reply-signature">Signature</FieldLabel>
        <Input
          id="reply-signature"
          maxLength={MAX_SIGNATURE}
          readOnly={!isAdmin}
          placeholder="Example: The Acme HR Team"
          value={signature}
          onChange={(e) => setSignature(e.target.value)}
        />
      </div>

      <ErrorNote error={save.error} />
      {isAdmin ? (
        <div className="flex items-center gap-3">
          <Button type="submit" loading={save.isPending} disabled={!dirty}>
            Save
          </Button>
          {save.isSuccess && !dirty && (
            <span role="status" className="text-sm text-ink-muted">
              Saved
            </span>
          )}
        </div>
      ) : (
        <p className="text-sm text-ink-muted">Only a workspace admin can change these.</p>
      )}
    </form>
  );
}

export function ReplySettingsCard({ ws, isAdmin }: { ws: string; isAdmin: boolean }) {
  const settings = useReplySettings(ws);
  return (
    <Card aria-labelledby="reply-settings-heading" role="region" className="flex flex-col gap-4">
      <CardTitle id="reply-settings-heading">Replies</CardTitle>
      {settings.isLoading && <p className="text-sm text-ink-muted">Loading…</p>}
      <ErrorNote error={settings.error} />
      {settings.data && <ReplySettingsForm ws={ws} initial={settings.data} isAdmin={isAdmin} />}
    </Card>
  );
}
