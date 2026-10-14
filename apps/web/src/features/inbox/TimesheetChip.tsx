import type { TimesheetChip as Record } from '../../api/types';
import { TextLink } from '../../components/ui';
import { hoursLabel, periodLabel } from '../hrms/timesheetFormat';

/** "Timesheet recorded · Sep 2026 · 168 h", linking to the record in HRMS Timesheets. */
export function TimesheetChip({ ws, records }: { ws: string; records: Record[] }) {
  const shown = records.filter((r) => r.status !== 'replaced');
  if (shown.length === 0) return null;
  return (
    <div className="flex flex-wrap gap-2" aria-label="Timesheets recorded from this email">
      {shown.map((r) => (
        <TextLink
          key={r.id}
          to={`/w/${ws}/timesheets?record=${r.id}`}
          className="inline-flex items-center gap-1 rounded-sm border border-rule-strong bg-raised px-2 py-1 text-xs font-semibold no-underline"
        >
          {r.status === 'needs_check' ? 'Timesheet needs a check' : 'Timesheet recorded'} · {periodLabel(r.period_start, r.period_end)}
          {r.total_hours !== null && ` · ${hoursLabel(r.total_hours)}`}
        </TextLink>
      ))}
    </div>
  );
}
