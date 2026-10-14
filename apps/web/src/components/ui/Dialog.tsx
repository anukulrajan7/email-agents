import { useEffect, useId, useRef, type KeyboardEvent, type ReactNode } from 'react';
import { cn } from '../../lib/cn';
import { ErrorNote } from '../ErrorNote';
import { Button } from './Button';

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), textarea:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])';

export function Dialog({
  title,
  titleId,
  variant = 'dialog',
  side = 'right',
  dismissible = true,
  onClose,
  className,
  children,
}: {
  title: string;
  titleId?: string;
  variant?: 'dialog' | 'drawer';
  side?: 'left' | 'right';
  dismissible?: boolean;
  onClose: () => void;
  className?: string;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const autoId = useId();
  const headingId = titleId ?? `${autoId}-title`;

  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>('button')?.focus();
    const { overflow } = document.body.style;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = overflow;
      trigger?.focus();
    };
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      if (dismissible) onClose();
      return;
    }
    if (e.key !== 'Tab' || !ref.current) return;
    const items = Array.from(ref.current.querySelectorAll<HTMLElement>(FOCUSABLE));
    const first = items[0];
    const last = items[items.length - 1];
    if (!first || !last) return;
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

  const drawer = variant === 'drawer';
  return (
    <div
      className={cn(
        'dialog-backdrop fixed inset-0 z-50 flex bg-ink/40',
        drawer ? (side === 'left' ? 'justify-start' : 'justify-end') : 'items-center justify-center p-4',
      )}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && dismissible) onClose();
      }}
    >
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={headingId}
        onKeyDown={onKeyDown}
        className={cn(
          'flex flex-col gap-4 border border-rule-strong bg-raised p-6 shadow-[0_24px_48px_-24px_rgba(28,27,24,0.45)]',
          drawer
            ? cn(
                'h-full w-full max-w-xl overflow-y-auto border-y-0',
                side === 'left' ? 'nav-drawer max-w-[20rem] border-l-0' : 'dialog-drawer border-r-0',
              )
            : 'dialog-panel max-h-[calc(100dvh-2rem)] w-full max-w-md overflow-y-auto',
          className,
        )}
      >
        <div className="flex items-start justify-between gap-4">
          <h2 id={headingId} className="type-display text-2xl tracking-[-0.01em] break-words">
            {title}
          </h2>
          {drawer && (
            <Button variant="ghost" size="sm" onClick={onClose}>
              Close
            </Button>
          )}
        </div>
        {children}
      </div>
    </div>
  );
}

export function ConfirmDialog({
  title,
  children,
  confirmLabel,
  cancelLabel = 'Cancel',
  tone = 'danger',
  busy = false,
  confirmDisabled = false,
  error,
  onConfirm,
  onCancel,
}: {
  title: string;
  children?: ReactNode;
  confirmLabel: string;
  cancelLabel?: string;
  tone?: 'danger' | 'primary';
  busy?: boolean;
  confirmDisabled?: boolean;
  error?: unknown;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  return (
    <Dialog title={title} onClose={onCancel} dismissible={!busy}>
      {children && <div className="text-sm text-ink-muted">{children}</div>}
      <ErrorNote error={error} />
      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="secondary" disabled={busy} onClick={onCancel}>
          {cancelLabel}
        </Button>
        <Button variant={tone} loading={busy} disabled={confirmDisabled} onClick={onConfirm}>
          {confirmLabel}
        </Button>
      </div>
    </Dialog>
  );
}
