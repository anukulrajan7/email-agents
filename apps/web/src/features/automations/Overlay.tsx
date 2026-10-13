import { useEffect, useRef, type KeyboardEvent, type ReactNode } from 'react';
import { Button } from '../../components/ui';
import { cn } from '../../lib/cn';

const FOCUSABLE = 'button:not([disabled]), [href], input, textarea, select, [tabindex]:not([tabindex="-1"])';

export function Overlay({
  title,
  titleId,
  variant = 'dialog',
  onClose,
  children,
}: {
  title: string;
  titleId: string;
  variant?: 'dialog' | 'drawer';
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>('button')?.focus();
    return () => trigger?.focus();
  }, []);

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onClose();
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
    <div className={cn('fixed inset-0 z-50 flex bg-ink/40', drawer ? 'justify-end' : 'items-center justify-center p-4')}>
      <div
        ref={ref}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onKeyDown}
        className={cn(
          'flex flex-col gap-4 border border-rule-strong bg-raised p-5',
          drawer ? 'h-full w-full max-w-xl overflow-y-auto border-y-0 border-r-0' : 'w-full max-w-md',
        )}
      >
        <div className="flex items-start justify-between gap-4">
          <h2 id={titleId} className="type-display text-2xl tracking-[-0.01em]">
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
