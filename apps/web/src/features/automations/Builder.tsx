import { useEffect, useRef, useState, type FormEvent } from 'react';
import type { Label } from '../../api/types';
import { Button, Card, Input, Label as FieldLabel } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { ACTION_OPTIONS, ActionRow, defaultAction } from './ActionRow';
import { ConditionRow, defaultCondition } from './ConditionRow';
import { AutomationSentence } from './Sentence';
import type { Action, ActionType, Automation, AutomationInput, Condition } from './types';
import { useTestAutomation } from './hooks';
import { needsEarlierDraft, validateAutomation } from './validate';

type Keyed<T> = { id: number; value: T };

function cleanAction(a: Action): Action {
  if (a.type === 'draft_reply') {
    const instructions = a.instructions?.trim();
    return instructions ? { type: a.type, instructions } : { type: a.type };
  }
  if (a.type === 'create_review') {
    const title = a.title?.trim();
    return title ? { type: a.type, title } : { type: a.type };
  }
  return a;
}

function cleanCondition(c: Condition): Condition {
  return c.field === 'from' || c.field === 'subject' ? { ...c, value: c.value.trim() } : c;
}

function TestPanel({ ws, automationId }: { ws: string; automationId: string | undefined }) {
  const test = useTestAutomation(ws);
  const hintId = 'test-hint';
  return (
    <section aria-labelledby="test-heading" className="flex flex-col gap-2 border-t border-rule pt-4">
      <h3 id="test-heading" className="text-sm font-semibold text-ink">
        Try it out
      </h3>
      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant="secondary"
          size="sm"
          disabled={!automationId}
          aria-describedby={hintId}
          loading={test.isPending}
          onClick={() => automationId && test.mutate(automationId)}
        >
          Test on recent emails
        </Button>
        <p id={hintId} className="text-sm text-ink-muted">
          {automationId ? 'Checks your last 50 emails against the saved version. Nothing is sent or changed.' : 'Save this automation first, then you can test it on recent emails.'}
        </p>
      </div>
      <ErrorNote error={test.error} />
      {test.data && (
        <div role="status" className="flex flex-col gap-1 text-sm">
          {test.data.matched.length === 0 ? (
            <p className="text-ink-muted">None of your last 50 emails would have matched.</p>
          ) : (
            <>
              <p className="font-semibold text-ink">
                {test.data.matched.length} of your last 50 emails would have matched
              </p>
              <ul className="flex max-h-48 flex-col gap-1 overflow-y-auto">
                {test.data.matched.map((m) => (
                  <li key={m.message_id} className="truncate text-ink-muted" title={m.subject}>
                    {m.subject || '(no subject)'}
                  </li>
                ))}
              </ul>
            </>
          )}
        </div>
      )}
    </section>
  );
}

export function AutomationBuilder({
  ws,
  initial,
  labels,
  saving,
  error,
  onSave,
  onCancel,
}: {
  ws: string;
  initial?: Automation;
  labels: Label[];
  saving: boolean;
  error: unknown;
  onSave: (input: AutomationInput) => void;
  onCancel: () => void;
}) {
  const nextId = useRef(0);
  const keyed = <T,>(value: T): Keyed<T> => ({ id: nextId.current++, value });
  const [name, setName] = useState(initial?.name ?? '');
  const [conds, setConds] = useState<Keyed<Condition>[]>(() => (initial?.conditions ?? []).map(keyed));
  const [acts, setActs] = useState<Keyed<Action>[]>(() => (initial?.actions ?? []).map(keyed));
  const [attempted, setAttempted] = useState(false);
  const actionList = useRef<HTMLOListElement>(null);
  const focusRequest = useRef<{ id: number; dir: 'up' | 'down' } | undefined>(undefined);

  const activeLabels = labels.filter((l) => !l.archived);
  const labelName = (key: string) => labels.find((l) => l.key === key)?.name ?? key;

  const input: AutomationInput = {
    name: name.trim(),
    enabled: initial?.enabled ?? true,
    conditions: conds.map((c) => cleanCondition(c.value)),
    actions: acts.map((a) => cleanAction(a.value)),
  };
  const issues = validateAutomation(input);

  useEffect(() => {
    const req = focusRequest.current;
    if (!req) return;
    focusRequest.current = undefined;
    const row = actionList.current?.querySelector(`[data-action-id="${req.id}"]`);
    const preferred = row?.querySelector<HTMLButtonElement>(`[data-move="${req.dir}"]`);
    const other = row?.querySelector<HTMLButtonElement>(`[data-move="${req.dir === 'up' ? 'down' : 'up'}"]`);
    (preferred && !preferred.disabled ? preferred : other)?.focus();
  });

  const moveAction = (index: number, direction: -1 | 1) => {
    const target = index + direction;
    const item = acts[index];
    if (!item || target < 0 || target >= acts.length) return;
    focusRequest.current = { id: item.id, dir: direction === -1 ? 'up' : 'down' };
    setActs((list) => {
      const copy = [...list];
      copy.splice(index, 1);
      copy.splice(target, 0, item);
      return copy;
    });
  };

  const addAction = (type: ActionType) => setActs((list) => [...list, keyed(defaultAction(type))]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (saving) return;
    setAttempted(true);
    if (issues.length > 0) return;
    onSave(input);
  };

  const issueFor = (id: string) => (attempted ? issues.find((i) => i.id === id)?.message : undefined);
  const actionErrors = (i: number) =>
    attempted ? issues.filter((x) => x.id.startsWith(`action-${i}-`)).map((x) => x.message) : [];
  const nameError = issueFor('name');
  const actionsError = issueFor('actions');

  return (
    <form onSubmit={submit} className="grid items-start gap-8 xl:grid-cols-[minmax(0,1fr)_22rem]" noValidate>
      <div className="flex min-w-0 flex-col gap-8">
        <div className="flex max-w-md flex-col gap-1.5">
          <FieldLabel htmlFor="automation-name">Name</FieldLabel>
          <Input
            id="automation-name"
            maxLength={80}
            placeholder="Reply to timesheets"
            aria-invalid={nameError ? true : undefined}
            aria-describedby={nameError ? 'automation-name-error' : undefined}
            className={nameError ? 'border-danger' : undefined}
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
          {nameError && (
            <p id="automation-name-error" className="text-sm text-danger">
              {nameError}
            </p>
          )}
        </div>

        <section aria-labelledby="when-heading" className="flex flex-col gap-4 rounded-sm border border-rule bg-surface p-5">
          <div className="flex items-baseline gap-3">
            <span aria-hidden="true" className="font-mono text-xs font-semibold tracking-[0.14em] text-accent">
              01
            </span>
            <h2 id="when-heading" className="type-display text-2xl tracking-[-0.01em]">
              When an email arrives{conds.length > 0 ? '…' : ''}
            </h2>
          </div>
          {conds.length === 0 && (
            <p className="text-sm text-ink-muted">Right now this runs for every email. Add a condition to narrow it down, for example by label or sender.</p>
          )}
          {conds.length > 0 && (
            <ul className="flex flex-col gap-3" aria-label="Conditions">
              {conds.map((c, i) => (
                <ConditionRow
                  key={c.id}
                  index={i}
                  condition={c.value}
                  labels={activeLabels}
                  error={issueFor(`condition-${i}`)}
                  onChange={(next) => setConds((list) => list.map((x) => (x.id === c.id ? { ...x, value: next } : x)))}
                  onRemove={() => setConds((list) => list.filter((x) => x.id !== c.id))}
                />
              ))}
            </ul>
          )}
          <div>
            <Button variant="secondary" size="sm" onClick={() => setConds((list) => [...list, keyed(defaultCondition('label', activeLabels))])}>
              + Add a condition
            </Button>
          </div>
        </section>

        <section aria-labelledby="then-heading" className="flex flex-col gap-4 rounded-sm border border-rule bg-surface p-5">
          <div className="flex items-baseline gap-3">
            <span aria-hidden="true" className="font-mono text-xs font-semibold tracking-[0.14em] text-accent">
              02
            </span>
            <h2 id="then-heading" className="type-display text-2xl tracking-[-0.01em]">
              Then…
            </h2>
          </div>
          {acts.length === 0 && (
            <p className={actionsError ? 'text-sm text-danger' : 'text-sm text-ink-muted'}>
              {actionsError ?? 'Pick the first step below. Steps run in order, top to bottom.'}
            </p>
          )}
          {acts.length > 0 && (
            <ol ref={actionList} className="flex flex-col gap-3" aria-label="Steps">
              {acts.map((a, i) => (
                <ActionRow
                  key={a.id}
                  actionId={a.id}
                  index={i}
                  count={acts.length}
                  action={a.value}
                  problem={needsEarlierDraft(input.actions, i) ? 'Add “Draft a reply” before this step.' : undefined}
                  errors={actionErrors(i)}
                  onChange={(next) => setActs((list) => list.map((x) => (x.id === a.id ? { ...x, value: next } : x)))}
                  onMove={(d) => moveAction(i, d)}
                  onRemove={() => setActs((list) => list.filter((x) => x.id !== a.id))}
                />
              ))}
            </ol>
          )}
          <div className="flex flex-col gap-2">
            <p className="kicker">{acts.length === 0 ? 'Add a step' : 'Add another step'}</p>
            <div role="group" aria-label="Add a step" className="flex flex-wrap gap-2">
              {ACTION_OPTIONS.map((o) => (
                <Button key={o.type} variant="secondary" size="sm" onClick={() => addAction(o.type)}>
                  + {o.label}
                </Button>
              ))}
            </div>
          </div>
        </section>

        <TestPanel ws={ws} automationId={initial?.id} />

        {attempted && issues.length > 0 && (
          <div role="alert" className="rounded-sm border-l-[3px] border-danger bg-danger-tint px-3 py-2 text-sm text-danger">
            <p className="font-semibold">Fix these before saving</p>
            <ul className="list-disc pl-5">
              {issues.map((issue) => (
                <li key={issue.id}>{issue.message}</li>
              ))}
            </ul>
          </div>
        )}
        <ErrorNote error={error} />

        <div className="flex flex-wrap gap-2 border-t border-rule pt-5">
          <Button type="submit" loading={saving}>
            {initial ? 'Save changes' : 'Create automation'}
          </Button>
          <Button variant="secondary" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </div>

      <aside aria-label="Preview" className="flex flex-col gap-3 xl:sticky xl:top-6">
        <Card className="flex flex-col gap-3 bg-raised p-5">
          <p className="kicker">In plain words</p>
          <AutomationSentence conditions={input.conditions} actions={input.actions} labelName={labelName} />
          <p className={issues.length === 0 ? 'border-t border-rule pt-3 text-sm text-success' : 'border-t border-rule pt-3 text-sm text-ink-muted'}>
            {issues.length === 0 ? 'Ready to save.' : `${issues.length} ${issues.length === 1 ? 'thing' : 'things'} left before you can save.`}
          </p>
        </Card>
      </aside>
    </form>
  );
}
