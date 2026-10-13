import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import { cn } from '../../lib/cn';

export type ToastTone = 'success' | 'error';
export type Notify = (tone: ToastTone, message: string) => void;

type ToastItem = { id: number; tone: ToastTone; message: string };

const ToastContext = createContext<Notify | undefined>(undefined);

const noop: Notify = () => undefined;

export function useToast(): Notify {
  return useContext(ToastContext) ?? noop;
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastItem[]>([]);
  const nextId = useRef(1);
  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);
  const notify = useCallback<Notify>(
    (tone, message) => {
      const id = nextId.current++;
      setToasts((t) => [...t.slice(-3), { id, tone, message }]);
      if (tone === 'success') setTimeout(() => dismiss(id), 6000);
    },
    [dismiss],
  );
  const value = useMemo(() => notify, [notify]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <ToastRegion toasts={toasts} onDismiss={dismiss} />
    </ToastContext.Provider>
  );
}

function ToastRegion({ toasts, onDismiss }: { toasts: ToastItem[]; onDismiss: (id: number) => void }) {
  return (
    <div
      role="status"
      aria-live="polite"
      className="pointer-events-none fixed inset-x-4 bottom-4 z-[60] flex flex-col items-end gap-2 sm:inset-x-auto sm:right-6 sm:bottom-6 sm:w-80"
    >
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cn(
            'toast-in pointer-events-auto flex w-full items-start gap-2 rounded-sm border border-l-[3px] border-rule-strong bg-raised px-3 py-2 text-sm shadow-[0_8px_24px_-12px_rgba(28,27,24,0.4)]',
            t.tone === 'success' ? 'border-l-success' : 'border-l-danger',
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
            className="-my-0.5 flex h-6 w-6 items-center justify-center rounded-xs text-ink-muted hover:bg-ink/5 hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
          >
            <span aria-hidden="true">×</span>
          </button>
        </div>
      ))}
    </div>
  );
}
