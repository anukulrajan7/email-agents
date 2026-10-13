import { Link } from 'react-router';
import { AgentByline } from './AgentAvatar';
import { personName } from './agentLabels';
import { CaseKindIcon, KIND_LABEL } from './icons';
import { CaseStatusChips } from './CaseStatusChips';
import { formatCents, wakeText } from './labels';
import type { CaseSummary } from './types';

export function CaseRow({ ws, item }: { ws: string; item: CaseSummary }) {
  return (
    <li className="flex gap-3 rounded-sm border border-rule bg-surface p-4">
      <span className="mt-0.5 text-ink-muted" title={KIND_LABEL[item.kind]}>
        <CaseKindIcon kind={item.kind} />
        <span className="sr-only">{KIND_LABEL[item.kind]}: </span>
      </span>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="min-w-0">
            <Link
              to={`/w/${ws}/agents/${item.id}`}
              className="type-display text-lg tracking-[-0.01em] break-words hover:text-accent hover:underline focus-visible:outline-2 focus-visible:outline-accent"
            >
              {item.title}
            </Link>
            <p className="text-sm break-words text-ink-muted">{item.goal}</p>
            {item.agent && (
              <p className="mt-1">
                <AgentByline agent={item.agent}>{item.working_for ? ` · working for ${personName(item.working_for)}` : ''}</AgentByline>
              </p>
            )}
          </div>
          <CaseStatusChips c={item} />
        </div>
        <p className="flex flex-wrap items-center gap-x-4 gap-y-1 font-mono text-xs text-ink-muted">
          <span>
            {item.steps} of {item.max_steps} steps
          </span>
          <span>{formatCents(item.spent_usd)}</span>
          {item.next_wake_at && item.status === 'waiting_event' && <span>checks again {wakeText(item.next_wake_at)}</span>}
        </p>
      </div>
    </li>
  );
}
