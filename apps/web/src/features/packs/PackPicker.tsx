import type { Pack } from '../../api/types';
import { cn } from '../../lib/cn';

type Props = {
  packs: Pack[];
  selected: string[];
  onChange: (keys: string[]) => void;
  disabled?: boolean;
};

export function PackPicker({ packs, selected, onChange, disabled }: Props) {
  const toggle = (key: string) => onChange(selected.includes(key) ? selected.filter((k) => k !== key) : [...selected, key]);
  return (
    <div className="grid auto-rows-fr gap-3 sm:grid-cols-2 lg:grid-cols-3">
      {packs.map((p) => {
        const on = selected.includes(p.key);
        return (
          <label
            key={p.key}
            className={cn(
              'flex h-full cursor-pointer flex-col gap-1.5 rounded-sm border p-4 text-sm motion-safe:transition-colors',
              'has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent',
              disabled && 'cursor-not-allowed opacity-60',
              on ? 'border-ink bg-surface shadow-[inset_0_3px_0_var(--color-accent)]' : 'border-rule-strong hover:border-ink',
            )}
          >
            <span className="flex items-center gap-2 font-semibold">
              <input
                type="checkbox"
                value={p.key}
                checked={on}
                disabled={disabled}
                onChange={() => toggle(p.key)}
                className="accent-[var(--color-accent)]"
              />
              {p.name}
            </span>
            <span className="text-ink-muted">{p.description}</span>
            <span className="mt-auto pt-1 text-xs text-ink-muted">{p.labels.map((l) => l.name).join(' · ')}</span>
          </label>
        );
      })}
    </div>
  );
}
