import { useEffect, useRef } from 'react';
import { Button } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';

const FOCUSABLE = 'button:not([disabled]), [href], input, textarea, select, [tabindex]:not([tabindex="-1"])';

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
  const ref = useRef<HTMLDivElement>(null);
  const preview = body.split('\n').slice(0, 3).join('\n');
  const hasMore = body.split('\n').length > 3;

  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>('button')?.focus();
    return () => trigger?.focus();
  }, []);

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      if (!sending) onCancel();
      return;
    }
    if (e.key !== 'Tab' || !ref.current) return;
    const items = Array.from(ref.current.querySelectorAll<HTMLElement>(FOCUSABLE));
    if (items.length === 0) return;
    const first = items[0]!;
    const last = items[items.length - 1]!;
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    } else if (!ref.current.contains(document.activeElement)) {
      e.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-ink/40 p-4">
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby="send-dialog-title"
        onKeyDown={onKeyDown}
        className="flex w-full max-w-md flex-col gap-3 border border-rule-strong bg-raised p-5"
      >
        <h4 id="send-dialog-title" className="type-display text-xl">
          Send this reply?
        </h4>
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
        <p className="ruled whitespace-pre-wrap break-words bg-surface px-3 text-sm">
          {preview}
          {hasMore ? ' …' : ''}
        </p>
        <p className="text-sm text-ink-muted">Sends from your connected Gmail, in the same conversation. This can't be undone.</p>
        <ErrorNote error={error} />
        <div className="flex justify-end gap-2">
          <Button variant="secondary" disabled={sending} onClick={onCancel}>
            Cancel
          </Button>
          <Button loading={sending} onClick={onConfirm}>
            Send now
          </Button>
        </div>
      </div>
    </div>
  );
}
