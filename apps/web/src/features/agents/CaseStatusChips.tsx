import { Badge } from '../../components/ui';
import { outcomeLabel, outcomeTone, STATUS_LABEL, STATUS_TONE } from './labels';
import type { CaseSummary } from './types';

export function CaseStatusChips({ c }: { c: Pick<CaseSummary, 'status' | 'outcome'> }) {
  return (
    <span className="flex flex-wrap items-center gap-1.5">
      <Badge tone={STATUS_TONE[c.status]}>{STATUS_LABEL[c.status]}</Badge>
      {c.outcome && (
        <Badge tone={outcomeTone(c.outcome)}>
          <span className="sr-only">Outcome: </span>
          {outcomeLabel(c.outcome)}
        </Badge>
      )}
    </span>
  );
}
