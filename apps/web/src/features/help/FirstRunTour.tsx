import { useState } from 'react';
import { Button } from '../../components/ui';
import { isTourDone, markTourDone } from './tourStorage';

const STEPS = [
  { title: 'Meet your agents', text: 'Each agent handles one kind of email. Meera leads the team, and you have a personal agent that only sees your own mail.' },
  { title: 'They ask before sending', text: 'Until an agent has earned your trust it only drafts. Anything it wants to send shows up under Agents for your OK.' },
  { title: 'Teach as you go', text: 'Edit a draft, fix a label or pin a fact in Memory. Your agents learn from every correction.' },
  { title: 'Ask Meera anything', text: 'The chat button in the corner answers questions about your mail and how things work here.' },
];

export function FirstRunTour() {
  const [open, setOpen] = useState(() => !isTourDone());
  const [step, setStep] = useState(0);
  if (!open) return null;
  const current = STEPS[step];
  if (!current) return null;
  const last = step === STEPS.length - 1;
  const close = () => {
    markTourDone();
    setOpen(false);
  };

  return (
    <section
      role="region"
      aria-label="Quick tour"
      className="rise fixed bottom-4 left-4 z-40 flex w-[min(21rem,calc(100vw-2rem))] flex-col gap-3 rounded-sm border border-ink bg-raised p-4 shadow-[0_18px_32px_-20px_rgba(28,27,24,0.5)]"
    >
      <p className="kicker">
        Quick tour · {step + 1} of {STEPS.length}
      </p>
      <p className="type-display text-xl leading-tight tracking-[-0.01em]">{current.title}</p>
      <p className="text-sm leading-relaxed text-ink-muted">{current.text}</p>
      <div className="flex items-center justify-between gap-2">
        <Button variant="ghost" size="sm" onClick={close}>
          Skip tour
        </Button>
        <div className="flex gap-2">
          {step > 0 && (
            <Button variant="secondary" size="sm" onClick={() => setStep(step - 1)}>
              Back
            </Button>
          )}
          <Button size="sm" onClick={() => (last ? close() : setStep(step + 1))}>
            {last ? 'Got it' : 'Next'}
          </Button>
        </div>
      </div>
    </section>
  );
}
