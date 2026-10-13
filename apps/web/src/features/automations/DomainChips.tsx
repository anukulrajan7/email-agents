import { useState, type KeyboardEvent } from 'react';
import { Button, Input, Label as FieldLabel } from '../../components/ui';
import { domainError, normalizeDomain } from './validate';

export function DomainChips({ id, domains, onChange }: { id: string; domains: string[]; onChange: (domains: string[]) => void }) {
  const [text, setText] = useState('');
  const [error, setError] = useState<string | undefined>();
  const errorId = `${id}-error`;

  const commit = () => {
    if (text.trim() === '') return;
    const problem = domainError(text);
    if (problem) {
      setError(problem);
      return;
    }
    const domain = normalizeDomain(text);
    if (!domains.includes(domain)) onChange([...domains, domain]);
    setText('');
    setError(undefined);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault();
      commit();
    } else if (e.key === 'Backspace' && text === '' && domains.length > 0) {
      onChange(domains.slice(0, -1));
    }
  };

  return (
    <div className="flex flex-col gap-1">
      <FieldLabel htmlFor={id}>Only auto-send to these sender domains</FieldLabel>
      {domains.length > 0 && (
        <ul className="flex flex-wrap gap-1" aria-label="Allowed domains">
          {domains.map((d) => (
            <li key={d} className="inline-flex items-center gap-1 rounded-sm border border-rule-strong bg-neutral-tint py-px pr-1 pl-2 font-mono text-xs text-ink">
              {d}
              <button
                type="button"
                aria-label={`Remove ${d}`}
                onClick={() => onChange(domains.filter((x) => x !== d))}
                className="flex h-5 w-5 items-center justify-center rounded-xs text-ink-muted hover:bg-ink/10 hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
              >
                <span aria-hidden="true">×</span>
              </button>
            </li>
          ))}
        </ul>
      )}
      <div className="flex gap-2">
        <Input
          id={id}
          className="max-w-xs"
          placeholder="acme.com"
          autoComplete="off"
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : `${id}-hint`}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          onBlur={commit}
        />
        <Button size="sm" variant="secondary" className="self-center" onClick={commit}>
          Add domain
        </Button>
      </div>
      {error ? (
        <p id={errorId} role="alert" className="text-sm text-danger">
          {error}
        </p>
      ) : (
        <p id={`${id}-hint`} className="text-sm text-ink-muted">
          The sender’s email domain must match one of these, or the reply is skipped.
        </p>
      )}
    </div>
  );
}
