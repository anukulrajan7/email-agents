import { useId, useState, type FormEvent } from 'react';
import { Link } from 'react-router';
import type { Memory, MemoryKind, MemoryScope } from '../../api/types';
import { Badge, Button, Label, Select, Textarea, useToast, type Tone } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { humanize } from '../../lib/format';
import { cn } from '../../lib/cn';
import { useCreateMemory, useUpdateMemory } from './hooks';

export const MEMORY_MAX = 500;
const KINDS: MemoryKind[] = ['fact', 'preference', 'style'];
const kindTone: Record<MemoryKind, Tone> = { fact: 'info', preference: 'success', style: 'warning' };

export function KindBadge({ kind }: { kind: MemoryKind }) {
  return <Badge tone={kindTone[kind]}>{kind}</Badge>;
}

function Counter({ id, length }: { id: string; length: number }) {
  return (
    <p id={id} className={cn('font-mono text-xs', length > MEMORY_MAX ? 'text-danger' : 'text-ink-muted')}>
      {length}/{MEMORY_MAX}
    </p>
  );
}

export function isValidContent(text: string) {
  const n = text.trim().length;
  return n >= 1 && text.length <= MEMORY_MAX;
}

export function MemoryForm({
  ws,
  scope,
  contactEmail,
  textareaLabel,
  submitLabel = 'Add note',
  rows = 3,
}: {
  ws: string;
  scope: MemoryScope;
  contactEmail?: string;
  textareaLabel: string;
  submitLabel?: string;
  rows?: number;
}) {
  const uid = useId();
  const [kind, setKind] = useState<MemoryKind>('fact');
  const [content, setContent] = useState('');
  const create = useCreateMemory(ws);
  const notify = useToast();

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!isValidContent(content)) return;
    create.mutate(
      { scope, kind, content: content.trim(), ...(scope === 'contact' && contactEmail ? { contact_email: contactEmail } : {}) },
      {
        onSuccess: () => {
          setContent('');
          notify('success', 'Saved. The assistant will use this from now on.');
        },
      },
    );
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${uid}-kind`}>Kind</Label>
        <Select id={`${uid}-kind`} value={kind} onChange={(e) => setKind(e.target.value as MemoryKind)} className="w-40">
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {humanize(k)}
            </option>
          ))}
        </Select>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${uid}-content`}>{textareaLabel}</Label>
        <Textarea
          id={`${uid}-content`}
          rows={rows}
          value={content}
          aria-describedby={`${uid}-count`}
          aria-invalid={content.length > MEMORY_MAX}
          onChange={(e) => setContent(e.target.value)}
        />
        <Counter id={`${uid}-count`} length={content.length} />
      </div>
      <ErrorNote error={create.error} />
      <Button type="submit" className="self-start" loading={create.isPending} disabled={!isValidContent(content)}>
        {submitLabel}
      </Button>
    </form>
  );
}

export function MemoryItem({ ws, memory }: { ws: string; memory: Memory }) {
  const uid = useId();
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(memory.content);
  const update = useUpdateMemory(ws);
  const notify = useToast();

  const save = () =>
    update.mutate({ id: memory.id, content: text.trim() }, { onSuccess: () => setEditing(false) });

  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-2">
        <KindBadge kind={memory.kind} />
        {memory.scope === 'contact' && memory.contact_email && (
          <Link
            to={`/w/${ws}/memory/contacts/${encodeURIComponent(memory.contact_email)}`}
            className="rounded-xs font-mono text-xs text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover"
          >
            {memory.contact_email}
          </Link>
        )}
      </div>
      {editing ? (
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-edit`}>Edit note</Label>
          <Textarea
            id={`${uid}-edit`}
            rows={3}
            value={text}
            aria-describedby={`${uid}-count`}
            aria-invalid={text.length > MEMORY_MAX}
            onChange={(e) => setText(e.target.value)}
          />
          <Counter id={`${uid}-count`} length={text.length} />
          <div className="mt-1 flex gap-2">
            <Button size="sm" loading={update.isPending} disabled={!isValidContent(text) || text.trim() === memory.content} onClick={save}>
              Save
            </Button>
            <Button
              size="sm"
              variant="ghost"
              onClick={() => {
                setText(memory.content);
                setEditing(false);
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      ) : (
        <>
          <p className="text-[15px] break-words whitespace-pre-wrap">{memory.content}</p>
          <div className="flex gap-2">
            <Button size="sm" variant="secondary" onClick={() => setEditing(true)}>
              Edit
            </Button>
            <Button
              size="sm"
              variant="ghost"
              loading={update.isPending}
              onClick={() => update.mutate({ id: memory.id, status: 'archived' }, { onSuccess: () => notify('success', 'Note archived.') })}
            >
              Archive
            </Button>
          </div>
        </>
      )}
      <ErrorNote error={update.error} />
    </li>
  );
}
