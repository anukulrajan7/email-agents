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

type EmailTrigger = Extract<SpecTrigger, { type: 'email_received' }>;
type Rule = { base: SpecMatch; text: Record<ListKey, string> };

const isEmail = (t: SpecTrigger): t is EmailTrigger => t.type === 'email_received';

const toRule = (base: SpecMatch): Rule => ({
  base,
  text: {
    labels: base.labels.join(', '),
    subject_keywords: base.subject_keywords.join(', '),
    from_domains: base.from_domains.join(', '),
    attachment_types: base.attachment_types.join(', '),
  },
});

const toMatch = (r: Rule): SpecMatch => ({
  ...r.base,
  labels: split(r.text.labels),
  subject_keywords: split(r.text.subject_keywords),
  from_domains: split(r.text.from_domains),
  attachment_types: split(r.text.attachment_types),
});

const isEmptyMatch = (m: SpecMatch) =>
  !m.labels.length && !m.subject_keywords.length && !m.from_domains.length && !m.from_addresses.length && !m.attachment_types.length && m.needs_reply === null && m.priority === null;

function withRules(triggers: SpecTrigger[], matches: SpecMatch[]): SpecTrigger[] {
  const out: SpecTrigger[] = [];
  let next = 0;
  let lastEmail = -1;
  for (const t of triggers) {
    if (!isEmail(t)) out.push(t);
    else {
      const match = matches[next];
      if (match) {
        next += 1;
        out.push({ type: 'email_received', match });
        lastEmail = out.length - 1;
      }
    }
  }
  const extra: SpecTrigger[] = matches.slice(next).map((match) => ({ type: 'email_received', match }));
  out.splice(lastEmail === -1 ? out.length : lastEmail + 1, 0, ...extra);
  return out;
}

function Editor({ ws, agent, spec, canEdit }: { ws: string; agent: AgentSummary; spec: AgentSpec; canEdit: boolean }) {
  const save = useSaveSpec(ws, agent.id);
  const notify = useToast();
  const original = spec.triggers.filter(isEmail);
  const [rules, setRules] = useState<Rule[]>(() => (original.length ? original.map((t) => toRule(t.match)) : [toRule(emptyMatch)]));
  const matches = rules.map(toMatch).filter((m, _, all) => !(original.length === 0 && all.length === 1 && isEmptyMatch(m)));
  const triggers = withRules(spec.triggers, matches);
  const dirty = JSON.stringify(triggers) !== JSON.stringify(spec.triggers);
  const hasEmpty = matches.some(isEmptyMatch);

  const edit = (i: number, key: ListKey, value: string) => setRules(rules.map((r, at) => (at === i ? { ...r, text: { ...r.text, [key]: value } } : r)));

  const submit = () => save.mutate({ ...spec, triggers }, { onSuccess: () => notify('success', 'Saved as a new version') });

  return (
    <div className="flex flex-col gap-4">
      <p className="text-sm text-ink-muted">
        This agent gets an email when it matches all the filled boxes of any one rule. Leave a box empty to ignore it. Separate several with commas.
      </p>
      {rules.map((rule, i) => (
        <fieldset key={i} className="flex flex-col gap-2 rounded-md border border-rule-strong p-3">
          <div className="flex items-center justify-between">
            <legend className="text-sm font-medium">Rule {i + 1}</legend>
            {canEdit && rules.length > 1 && (
              <Button variant="ghost" size="sm" onClick={() => setRules(rules.filter((_, at) => at !== i))} aria-label={`Remove rule ${i + 1}`}>
                Remove
              </Button>
            )}
          </div>
          <div className="grid gap-2 sm:grid-cols-2">
            {FIELDS.map((f) => (
              <div key={f.key} className="flex flex-col gap-1">
                <Label htmlFor={`trigger-${i}-${f.key}`}>{f.label}</Label>
                <Input id={`trigger-${i}-${f.key}`} value={rule.text[f.key]} placeholder={f.hint} disabled={!canEdit} onChange={(e) => edit(i, f.key, e.target.value)} />
              </div>
            ))}
          </div>
        </fieldset>
      ))}
      {canEdit && (
        <div>
          <Button variant="secondary" size="sm" onClick={() => setRules([...rules, toRule(emptyMatch)])}>
            Add rule
          </Button>
        </div>
      )}
      {hasEmpty && dirty && <p role="alert" className="text-sm text-danger">Fill in at least one box in every rule, or remove the empty rule. An empty rule would catch every email.</p>}
      <p className="text-sm text-ink-muted">Saving a change makes this helper ask first again until it earns its level back.</p>
      <ErrorNote error={save.error} />
      <div>
        <Button disabled={!canEdit || !dirty || hasEmpty} loading={save.isPending} onClick={submit}>
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
