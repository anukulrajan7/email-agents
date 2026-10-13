import { useId, useState, type ClipboardEvent, type KeyboardEvent } from 'react';
import { Label } from '../../components/ui';
import { fieldClass } from '../../components/ui/fieldStyles';
import { cn } from '../../lib/cn';
import { isEmail } from './validation';

const SPLIT = /[\s,;]+/;

export function EmailChipInput({
  label,
  hint,
  value,
  onChange,
  error,
}: {
  label: string;
  hint?: string;
  value: string[];
  onChange: (next: string[]) => void;
  error?: string | undefined;
}) {
  const uid = useId();
  const [draft, setDraft] = useState('');
  const [invalid, setInvalid] = useState<string | undefined>(undefined);

  const commit = (raw: string) => {
    const parts = raw.split(SPLIT).filter(Boolean);
    if (parts.length === 0) return;
    const next = [...value];
    const bad: string[] = [];
    for (const part of parts) {
      const email = part.toLowerCase();
      if (!isEmail(email)) bad.push(part);
      else if (!next.includes(email)) next.push(email);
    }
    onChange(next);
    setDraft(bad.join(' '));
    setInvalid(bad.length > 0 ? `${bad.join(', ')} ${bad.length === 1 ? 'is not a valid email address' : 'are not valid email addresses'}.` : undefined);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',' || e.key === ';' || e.key === ' ') {
      if (draft.trim() === '') {
        if (e.key === 'Enter') e.preventDefault();
        return;
      }
      e.preventDefault();
      commit(draft);
    } else if (e.key === 'Backspace' && draft === '' && value.length > 0) {
      onChange(value.slice(0, -1));
    }
  };

  const onPaste = (e: ClipboardEvent<HTMLInputElement>) => {
    const text = e.clipboardData.getData('text');
    if (!SPLIT.test(text.trim())) return;
    e.preventDefault();
    commit(`${draft} ${text}`);
  };

  const message = invalid ?? error;
  const describedBy = [hint ? `${uid}-hint` : undefined, message ? `${uid}-error` : undefined].filter(Boolean).join(' ') || undefined;

  return (
    <div className="flex flex-col gap-1">
      <Label htmlFor={`${uid}-input`}>{label}</Label>
      {hint && (
        <p id={`${uid}-hint`} className="text-sm text-ink-muted">
          {hint}
        </p>
      )}
      <div
        className={cn(fieldClass, 'flex min-h-10 flex-wrap items-center gap-1.5 py-1.5 focus-within:border-ink', message && 'border-danger')}
      >
        {value.map((email) => (
          <span key={email} className="inline-flex items-center gap-1 rounded-sm border border-rule-strong bg-neutral-tint py-0.5 pr-0.5 pl-2 font-mono text-xs text-ink">
            {email}
            <button
              type="button"
              aria-label={`Remove ${email}`}
              onClick={() => onChange(value.filter((v) => v !== email))}
              className="flex h-5 w-5 items-center justify-center rounded-xs text-ink-muted hover:bg-ink/10 hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
            >
              <span aria-hidden="true">×</span>
            </button>
          </span>
        ))}
        <input
          id={`${uid}-input`}
          type="text"
          inputMode="email"
          autoComplete="off"
          value={draft}
          aria-invalid={message ? true : undefined}
          aria-describedby={describedBy}
          onChange={(e) => {
            setDraft(e.target.value);
            setInvalid(undefined);
          }}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onBlur={() => commit(draft)}
          className="min-w-48 flex-1 bg-transparent py-0.5 text-sm outline-none placeholder:text-ink-muted"
          placeholder={value.length === 0 ? 'name@company.com' : ''}
        />
      </div>
      {message && (
        <p id={`${uid}-error`} role="alert" className="text-sm text-danger">
          {message}
        </p>
      )}
    </div>
  );
}
