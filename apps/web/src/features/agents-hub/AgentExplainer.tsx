import type { AgentSummary } from '../../api/types';
import { AUTONOMY_PLAIN, agentCopy } from './agentCopy';

const ROWS = [
  ['What it does', 'does'],
  ['When it acts', 'when'],
  ['On its own', 'alone'],
  ['Asks you', 'asks'],
  ['For example', 'example'],
] as const;

/** What / when / on its own vs asks you / example, in plain words and the agent's own voice. */
export function AgentExplainer({ agent }: { agent: AgentSummary }) {
  const copy = agentCopy(agent);
  const level = AUTONOMY_PLAIN[agent.autonomy.effective];
  const text = { ...copy, alone: level.alone, asks: level.asks };
  return (
    <dl className="grid grid-cols-1 gap-x-4 gap-y-2 text-sm leading-snug sm:grid-cols-[6.5rem_minmax(0,1fr)]" aria-label={`About ${agent.name}`}>
      {ROWS.map(([label, key]) => (
        <div key={key} className="contents">
          <dt className="kicker pt-0.5">{label}</dt>
          <dd className="text-ink">{text[key]}</dd>
        </div>
      ))}
    </dl>
  );
}
