import { useEffect, useId, useRef, useState } from 'react';
import { cn } from '../../lib/cn';
import { Button } from './Button';

export type MoreMenuItem = { label: string; onSelect: () => void; disabled?: boolean; loading?: boolean };

export function MoreMenu({ items, label = 'More' }: { items: MoreMenuItem[]; label?: string }) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open]);

  return (
    <div ref={root} className="relative">
      <Button variant="ghost" aria-haspopup="menu" aria-expanded={open} aria-controls={open ? menuId : undefined} onClick={() => setOpen((o) => !o)}>
        {label}
      </Button>
      {open && (
        <ul id={menuId} role="menu" className="absolute left-0 z-10 mt-1 flex min-w-48 flex-col border border-rule-strong bg-raised py-1 shadow-sm">
          {items.map((item) => (
            <li key={item.label} role="none">
              <button
                type="button"
                role="menuitem"
                disabled={item.disabled || item.loading}
                onClick={() => {
                  setOpen(false);
                  item.onSelect();
                }}
                className={cn(
                  'block w-full px-3 py-2 text-left text-sm text-ink hover:bg-ink/5 disabled:cursor-not-allowed disabled:text-ink-muted',
                  'focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent',
                )}
              >
                {item.label}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
