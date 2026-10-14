import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { Button, Input, Label, Skeleton, Textarea } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { PackPicker } from '../packs/PackPicker';
import { usePackCatalogue } from '../packs/hooks';
import { useCreateWorkspace } from '../auth/hooks';
import { HelpPopover } from '../help/HelpPopover';
import { AgentFace } from '../agents-hub/AgentFace';
import { GOALS, PACK_TEAM, suggestPacks } from './goals';

type Step = 'goal' | 'packs' | 'name';
const STEPS: { id: Step; label: string }[] = [
  { id: 'goal', label: 'Your goal' },
  { id: 'packs', label: 'Your team' },
  { id: 'name', label: 'Name it' },
];

export function GoalWizard() {
  const [step, setStep] = useState<Step>('goal');
  const [goal, setGoal] = useState('');
  const [picked, setPicked] = useState<string[]>([]);
  const [packs, setPacks] = useState<string[]>(['general']);
  const [name, setName] = useState('');
  const catalogue = usePackCatalogue();
  const create = useCreateWorkspace();
  const navigate = useNavigate();
  const index = STEPS.findIndex((s) => s.id === step);

  const toggleGoal = (id: string, label: string) => {
    const next = picked.includes(id) ? picked.filter((g) => g !== id) : [...picked, id];
    setPicked(next);
    setPacks(Array.from(new Set(['general', ...GOALS.filter((g) => next.includes(g.id)).map((g) => g.pack)])));
    if (!goal.trim()) setGoal(label);
  };

  const toPacks = () => {
    setPacks((cur) => Array.from(new Set([...cur, ...suggestPacks(goal)])));
    setStep('packs');
  };

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed || packs.length === 0) return;
    create.mutate(
      { name: trimmed, packs, goal: goal.trim() || undefined },
      { onSuccess: (w) => navigate(`/w/${w.id}/welcome`) },
    );
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-6" aria-labelledby="wizard-title">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <ol aria-label="Progress" className="flex gap-4">
            {STEPS.map((s, i) => (
              <li
                key={s.id}
                aria-current={i === index ? 'step' : undefined}
                className={cn('kicker border-b-2 pb-1', i === index ? 'border-accent text-ink' : 'border-transparent')}
              >
                {i + 1}. {s.label}
              </li>
            ))}
          </ol>
        </div>
        <HelpPopover topic="welcome" />
      </div>

      {step === 'goal' && (
        <div className="flex flex-col gap-4">
          <div>
            <h2 id="wizard-title" className="type-display text-3xl leading-tight tracking-[-0.02em]">
              What do you want help with?
            </h2>
            <p className="mt-1 text-sm text-ink-muted">Pick what fits, or say it in your own words. We switch on the right agents for you.</p>
          </div>
          <div role="group" aria-label="Common goals" className="flex flex-wrap gap-2">
            {GOALS.map((g) => {
              const on = picked.includes(g.id);
              return (
                <button
                  key={g.id}
                  type="button"
                  aria-pressed={on}
                  onClick={() => toggleGoal(g.id, g.label)}
                  className={cn(
                    'rounded-sm border px-3 py-2 text-sm font-semibold motion-safe:transition-colors',
                    'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
                    on ? 'border-ink bg-surface shadow-[inset_0_3px_0_var(--color-accent)]' : 'border-rule-strong hover:border-ink',
                  )}
                >
                  {g.label}
                </button>
              );
            })}
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="goal-text">In your own words (optional)</Label>
            <Textarea id="goal-text" rows={3} maxLength={1000} value={goal} placeholder="We are a small agency. I want replies drafted and invoices chased." onChange={(e) => setGoal(e.target.value)} />
          </div>
          <div>
            <Button type="button" onClick={toPacks}>
              Continue
            </Button>
          </div>
        </div>
      )}

      {step === 'packs' && (
        <div className="flex flex-col gap-4">
          <div>
            <h2 id="wizard-title" className="type-display text-3xl leading-tight tracking-[-0.02em]">
              Here is your team
            </h2>
            <p className="mt-1 text-sm text-ink-muted">Each pack brings its own agents. Change this any time in Settings.</p>
          </div>
          {catalogue.isLoading && <Skeleton className="h-28" />}
          <ErrorNote error={catalogue.error} />
          {catalogue.data && <PackPicker packs={catalogue.data.items} selected={packs} onChange={setPacks} />}
          <ul aria-label="Agents you will get" className="flex flex-wrap gap-x-6 gap-y-1 text-sm text-ink-muted">
            {packs.flatMap((p) => PACK_TEAM[p] ?? []).map((n) => (
              <li key={n}>{n}</li>
            ))}
            <li>Your personal agent</li>
          </ul>
          <div className="flex gap-2">
            <Button type="button" variant="secondary" onClick={() => setStep('goal')}>
              Back
            </Button>
            <Button type="button" disabled={packs.length === 0} onClick={() => setStep('name')}>
              Continue
            </Button>
          </div>
        </div>
      )}

      {step === 'name' && (
        <div className="flex flex-col gap-4">
          <div className="flex items-center gap-4">
            <AgentFace avatar="blossom" size="xl" />
            <div>
              <h2 id="wizard-title" className="type-display text-3xl leading-tight tracking-[-0.02em]">
                Last thing: a name
              </h2>
              <p className="mt-1 text-sm text-ink-muted">Meera will be your lead agent. What should we call this workspace?</p>
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="ws-name">Workspace name</Label>
            <Input id="ws-name" required placeholder="Acme Talent Desk" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <ErrorNote error={create.error} />
          <div className="flex gap-2">
            <Button type="button" variant="secondary" onClick={() => setStep('packs')}>
              Back
            </Button>
            <Button type="submit" loading={create.isPending} disabled={!name.trim()}>
              Create workspace
            </Button>
          </div>
        </div>
      )}
    </form>
  );
}
