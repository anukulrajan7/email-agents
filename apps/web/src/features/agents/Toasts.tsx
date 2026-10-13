import { useCallback, useState } from 'react';
import { cn } from '../../lib/cn';

export type Toast = { id: number; tone: 'success' | 'error'; message: string };

let nextId = 1;

export function useToasts() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);
  const push = useCallback(
    (tone: Toast['tone'], message: string) => {
      const id = nextId++;
      setToasts((t) => [...t, { id, tone, message }]);
      if (tone === 'success') setTimeout(() => dismiss(id), 6000);
    },
    [dismiss],
  );
  return { toasts, push, dismiss };
}

export function ToastRegion({ toasts, onDismiss }: { toasts: Toast[]; onDismiss: (id: number) => void }) {
  return (
    <div role="status" aria-live="polite" className="pointer-events-none fixed right-6 bottom-6 z-50 flex w-80 flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cn(
            'pointer-events-auto flex items-start gap-2 rounded-sm border-l-[3px] bg-raised px-3 py-2 text-sm shadow-[0_8px_24px_-12px_rgba(28,27,24,0.4)]',
            t.tone === 'success' ? 'border-success' : 'border-danger',
          )}
        >
          <span aria-hidden="true" className={cn('font-bold', t.tone === 'success' ? 'text-success' : 'text-danger')}>
            {t.tone === 'success' ? '✓' : '!'}
          </span>
          <span className="sr-only">{t.tone === 'success' ? 'Done: ' : 'Problem: '}</span>
          <span className="min-w-0 flex-1 text-ink">{t.message}</span>
          <button
            type="button"
            onClick={() => onDismiss(t.id)}
            aria-label="Dismiss message"
            className="rounded-xs px-1 text-ink-muted hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
          >
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
