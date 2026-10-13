import { useId, useState, type FormEvent } from 'react';
import { Button, Input, Label } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useCreateMemory } from './hooks';
import { MEMORY_MAX, isValidContent } from './MemoryParts';

export function TeachFact({ ws }: { ws: string }) {
  const uid = useId();
  const [content, setContent] = useState('');
  const create = useCreateMemory(ws);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!isValidContent(content)) return;
    create.mutate({ scope: 'workspace', kind: 'fact', content: content.trim() }, { onSuccess: () => setContent('') });
  };

  return (
    <details className="text-sm text-ink-muted">
      <summary className="w-fit cursor-pointer rounded-xs py-1 font-semibold hover:text-ink">Teach a fact</summary>
      <form onSubmit={submit} className="mt-2 flex flex-col gap-2">
        <Label htmlFor={`${uid}-fact`}>Something the assistant should know</Label>
        <div className="flex flex-wrap items-center gap-2">
          <Input
            id={`${uid}-fact`}
            className="max-w-xl flex-1"
            value={content}
            maxLength={MEMORY_MAX}
            onChange={(e) => setContent(e.target.value)}
          />
          <Button type="submit" size="sm" variant="secondary" loading={create.isPending} disabled={!isValidContent(content)}>
            Add fact
          </Button>
        </div>
        <ErrorNote error={create.error} />
      </form>
    </details>
  );
}
