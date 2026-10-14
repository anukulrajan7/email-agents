import { useEffect, useId, useRef, useState } from 'react';
import { Button } from '../../components/ui';
import { AssistantPanel } from './AssistantPanel';
import { useAssistantName } from './hooks';

export function AskMeera({ ws }: { ws: string }) {
  const assistantName = useAssistantName(ws);
  const [open, setOpen] = useState(false);
  const [mounted, setMounted] = useState(false);
  const panelRef = useRef<HTMLElement>(null);
  const triggerId = useId();

  const focusInput = () => panelRef.current?.querySelector<HTMLInputElement>('input')?.focus();

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setMounted(true);
        setOpen(true);
        if (open) focusInput();
      } else if (e.key === 'Escape' && open) {
        setOpen(false);
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, [open]);

  useEffect(() => {
    if (open) focusInput();
    else if (panelRef.current?.contains(document.activeElement)) document.getElementById(triggerId)?.focus();
  }, [open, triggerId]);

  return (
    <>
      <Button
        id={triggerId}
        data-guide="ask-meera"
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => {
          setMounted(true);
          setOpen((o) => !o);
        }}
        className="fixed right-4 bottom-4 z-40 shadow-[0_8px_24px_-12px_rgba(28,27,24,0.5)]"
      >
        Ask {assistantName}
      </Button>
      {mounted && (
        <section
          ref={panelRef}
          role="dialog"
          aria-label={`Ask ${assistantName}`}
          hidden={!open}
          className="fixed inset-x-2 bottom-16 z-50 h-[34rem] max-h-[calc(100dvh-5rem)] flex-col border border-rule-strong bg-surface shadow-[0_16px_40px_-16px_rgba(28,27,24,0.55)] sm:inset-x-auto sm:right-4 sm:w-[24rem] [&:not([hidden])]:flex"
        >
          <AssistantPanel ws={ws} onClose={() => setOpen(false)} />
        </section>
      )}
    </>
  );
}
