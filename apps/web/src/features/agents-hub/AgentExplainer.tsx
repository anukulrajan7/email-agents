import { useId, useState } from 'react';
import type { AgentSummary } from '../../api/types';
import { AUTONOMY_PLAIN, agentCopy } from './agentCopy';

const ROWS = [
  ['What it does', 'does'],
  ['When it acts', 'when'],
  ['On its own', 'alone'],
  ['Asks you', 'asks'],
  ['For example', 'example'],
] as const;

/** What / when / on its own vs asks you / example, in plain words and the agent's own voice. Collapsed behind "How I work". */
export function AgentExplainer({ agent }: { agent: AgentSummary }) {
  const [open, setOpen] = useState(false);
  const id = useId();
  const copy = agentCopy(agent);
  const level = AUTONOMY_PLAIN[agent.autonomy.effective];
  const text = { ...copy, alone: level.alone, asks: level.asks };
  return (
    <div className="flex flex-col gap-2">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((o) => !o)}
        className="self-start rounded-sm text-[13px] font-semibold text-accent underline underline-offset-[3px] focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent"
      >
        How I work
      </button>
      {open && (
        <dl id={id} className="grid grid-cols-1 gap-x-4 gap-y-1 text-sm leading-snug sm:grid-cols-[6.5rem_minmax(0,1fr)]" aria-label={`About ${agent.name}`}>
          {ROWS.map(([label, key]) => (
            <div key={key} className="contents">
              <dt className="kicker pt-0.5">{label}</dt>
              <dd className="text-ink">{text[key]}</dd>
            </div>
          ))}
        </dl>
      )}
    </div>
  );
}
