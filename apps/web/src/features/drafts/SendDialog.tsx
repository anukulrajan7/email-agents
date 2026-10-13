import { Button, Dialog } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';

export function SendDialog({
  to,
  subject,
  body,
  sending,
  error,
  onCancel,
  onConfirm,
}: {
  to: string;
  subject: string;
  body: string;
  sending: boolean;
  error: unknown;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const lines = body.split('\n');
  const preview = lines.slice(0, 3).join('\n');
  const hasMore = lines.length > 3;

  return (
    <Dialog title="Send this reply?" titleId="send-dialog-title" dismissible={!sending} onClose={onCancel}>
      <dl className="flex flex-col gap-1 text-sm">
        <div>
          <dt className="inline font-semibold">To: </dt>
          <dd className="inline break-words">{to}</dd>
        </div>
        <div>
          <dt className="inline font-semibold">Subject: </dt>
          <dd className="inline break-words">{subject}</dd>
        </div>
      </dl>
      <p className="ruled border border-rule bg-surface px-3 text-sm break-words whitespace-pre-wrap">
        {preview}
        {hasMore ? ' …' : ''}
      </p>
      <p className="text-sm text-ink-muted">Sends from your connected Gmail, in the same conversation. This can&apos;t be undone.</p>
      <ErrorNote error={error} />
      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="secondary" disabled={sending} onClick={onCancel}>
          Cancel
        </Button>
        <Button loading={sending} onClick={onConfirm}>
          Send now
        </Button>
      </div>
    </Dialog>
  );
}
