import { useRef, type KeyboardEvent } from 'react';
import { cn } from '../../lib/cn';
import { AgentFace } from './AgentFace';
import { AVATARS, avatarValue } from './avatars';

/** `value` is an avatar key (e.g. "owl"); `onChange` receives the value to store on the agent (e.g. "fluent:owl"). */
export function AvatarPicker({
  label,
  value,
  disabled,
  onChange,
}: {
  label: string;
  value: string | undefined;
  disabled?: boolean;
  onChange: (stored: string) => void;
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const current = Math.max(0, AVATARS.findIndex((a) => a.key === value));

  const onKeyDown = (e: KeyboardEvent) => {
    const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (!step || disabled) return;
    e.preventDefault();
    const next = (current + step + AVATARS.length) % AVATARS.length;
    const choice = AVATARS[next];
    if (!choice) return;
    onChange(avatarValue(choice.key));
    refs.current[next]?.focus();
  };

  return (
    <div role="radiogroup" aria-label={label} onKeyDown={onKeyDown} className="grid grid-cols-4 gap-2 sm:grid-cols-8">
      {AVATARS.map((a, i) => {
        const on = i === current && value !== undefined;
        return (
          <button
            key={a.key}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={on}
            aria-label={a.label}
            title={a.label}
            tabIndex={i === current ? 0 : -1}
            disabled={disabled}
            onClick={() => onChange(avatarValue(a.key))}
            className={cn(
              'group flex items-center justify-center rounded-sm border p-1 motion-safe:transition-colors',
              'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent disabled:cursor-not-allowed disabled:opacity-50',
              on ? 'border-ink bg-surface shadow-[inset_0_3px_0_var(--color-accent)]' : 'border-rule-strong hover:border-ink',
            )}
          >
            <AgentFace avatar={a.key} size="md" index={i} />
          </button>
        );
      })}
    </div>
  );
}
