import { useState } from 'react';
import { Button, Input, Label, Skeleton, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import type { AgentSpec, AgentSummary, SpecMatch, SpecTrigger } from '../../api/types';
import { useAgentDetail, useSaveSpec } from './hooks';

type ListKey = 'labels' | 'subject_keywords' | 'from_domains' | 'attachment_types';

const FIELDS: { key: ListKey; label: string; hint: string }[] = [
  { key: 'labels', label: 'Labels', hint: 'timesheet, leave' },
  { key: 'subject_keywords', label: 'Subject words', hint: 'onboarding, new joiner' },
  { key: 'from_domains', label: 'Sender domains', hint: 'acme.com' },
  { key: 'attachment_types', label: 'Attachment types', hint: 'pdf, xlsx' },
];

const split = (text: string): string[] => [...new Set(text.split(',').map((t) => t.trim()).filter(Boolean))];

const emptyMatch: SpecMatch = { labels: [], from_domains: [], from_addresses: [], subject_keywords: [], needs_reply: null, priority: null, attachment_types: [] };

const isEmail = (t: SpecTrigger): t is Extract<SpecTrigger, { type: 'email_received' }> => t.type === 'email_received';

function Editor({ ws, agent, spec, canEdit }: { ws: string; agent: AgentSummary; spec: AgentSpec; canEdit: boolean }) {
  const save = useSaveSpec(ws, agent.id);
  const notify = useToast();
  const current = spec.triggers.find(isEmail)?.match ?? emptyMatch;
  const [text, setText] = useState<Record<ListKey, string>>(() => ({
    labels: current.labels.join(', '),
    subject_keywords: current.subject_keywords.join(', '),
    from_domains: current.from_domains.join(', '),
    attachment_types: current.attachment_types.join(', '),
  }));
  const match: SpecMatch = { ...current, labels: split(text.labels), subject_keywords: split(text.subject_keywords), from_domains: split(text.from_domains), attachment_types: split(text.attachment_types) };
  const dirty = JSON.stringify(match) !== JSON.stringify(current);

  const submit = () => {
    const email: SpecTrigger = { type: 'email_received', match };
    const at = spec.triggers.findIndex(isEmail);
    const others = spec.triggers.filter((t) => !isEmail(t));
    const triggers = at === -1 ? [email, ...others] : [...spec.triggers.slice(0, at).filter((t) => !isEmail(t)), email, ...spec.triggers.slice(at + 1).filter((t) => !isEmail(t))];
    save.mutate({ ...spec, triggers }, { onSuccess: () => notify('success', 'Saved as a new version') });
  };

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-ink-muted">This agent gets an email when it matches all the filled boxes. Leave a box empty to ignore it. Separate several with commas.</p>
      {FIELDS.map((f) => (
        <div key={f.key} className="flex flex-col gap-1">
          <Label htmlFor={`trigger-${f.key}`}>{f.label}</Label>
          <Input id={`trigger-${f.key}`} value={text[f.key]} placeholder={f.hint} disabled={!canEdit} onChange={(e) => setText({ ...text, [f.key]: e.target.value })} />
        </div>
      ))}
      <ErrorNote error={save.error} />
      <div>
        <Button disabled={!canEdit || !dirty} loading={save.isPending} onClick={submit}>
          Save triggers
        </Button>
      </div>
    </div>
  );
}

export function TriggersTab({ ws, agent, canEdit }: { ws: string; agent: AgentSummary; canEdit: boolean }) {
  const detail = useAgentDetail(ws, agent.id);
  if (detail.isLoading) return <Skeleton className="h-24" />;
  if (!detail.data) return <ErrorNote error={detail.error} />;
  return <Editor key={`${agent.id}-${detail.data.versions.find((v) => v.active)?.version ?? 0}`} ws={ws} agent={agent} spec={detail.data.spec} canEdit={canEdit} />;
}
