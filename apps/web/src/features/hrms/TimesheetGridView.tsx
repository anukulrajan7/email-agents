import { useMemo, useState } from 'react';
import { Badge, Button, EmptyState, Input, SegmentedControl, Select, SkeletonList, type Tone } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { useTimesheetGrid } from './hooks';
import { cellLabel, gridCsv, shortDate, statusText, weeksOf } from './timesheetFormat';
import type { GridRow, GridStatus } from './types';

const TONE: Record<GridStatus, Tone> = { recorded: 'success', needs_check: 'warning', missing: 'danger', pending: 'neutral' };

function download(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: 'text/csv;charset=utf-8' }));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

/** People by days, like a spreadsheet: hours per day, a total and a status. Click a row to see its records. */
export function TimesheetGridView({ ws, onOpen }: { ws: string; onOpen: (row: GridRow) => void }) {
  const [picked, setPicked] = useState('');
  const [scope, setScope] = useState<'month' | 'week'>('month');
  const [week, setWeek] = useState(0);
  const [client, setClient] = useState('');
  const [q, setQ] = useState('');
  const grid = useTimesheetGrid(ws, picked || undefined);
  const data = grid.data;
  const month = picked || data?.month || '';

  const weeks = useMemo(() => weeksOf(data?.days ?? []), [data]);
  const shownWeek = Math.min(week, Math.max(weeks.length - 1, 0));
  const days = scope === 'week' ? (weeks[shownWeek] ?? []) : (data?.days ?? []);
  const clients = useMemo(() => [...new Set((data?.rows ?? []).map((r) => r.client).filter((c): c is string => Boolean(c)))].sort(), [data]);
  const rows = (data?.rows ?? []).filter(
    (r) => (!client || r.client === client) && (!q.trim() || `${r.name} ${r.employee_code ?? ''}`.toLowerCase().includes(q.trim().toLowerCase())),
  );
  const shownTotal = (r: GridRow) => (scope === 'week' ? days.reduce((sum, d) => sum + (r.cells[d.date] ?? 0), 0) : r.total);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end gap-3">
        <Input aria-label="Month" type="month" className="w-44" value={month} onChange={(e) => e.target.value && setPicked(e.target.value)} />
        <SegmentedControl
          label="Show"
          value={scope}
          onChange={setScope}
          options={[
            { value: 'month', label: 'Month' },
            { value: 'week', label: 'Week' },
          ]}
        />
        {scope === 'week' && weeks.length > 0 && (
          <Select aria-label="Week" className="w-52" value={shownWeek} onChange={(e) => setWeek(Number(e.target.value))}>
            {weeks.map((w, i) => (
              <option key={w[0]?.date} value={i}>
                {shortDate(w[0]?.date ?? '')} – {shortDate(w[w.length - 1]?.date ?? '')}
              </option>
            ))}
          </Select>
        )}
        <Select aria-label="Client or project" className="w-48" value={client} onChange={(e) => setClient(e.target.value)}>
          <option value="">All clients</option>
          {clients.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </Select>
        <Input aria-label="Search people" className="max-w-56" placeholder="Search name or ID" value={q} onChange={(e) => setQ(e.target.value)} />
        <Button variant="secondary" disabled={rows.length === 0} onClick={() => download(`timesheets-${month}${scope === 'week' ? `-week-${shownWeek + 1}` : ''}.csv`, gridCsv(days, rows, shownTotal))}>
          Export CSV
        </Button>
      </div>
      {data && (
        <p className="text-sm text-ink-muted">
          {data.summary.recorded} recorded · {data.summary.needs_check} need a check · {data.summary.missing} missing
          {data.summary.unresolved ? ` · ${data.summary.unresolved} unknown ${data.summary.unresolved === 1 ? 'name' : 'names'} (kept, not counted for anyone)` : ''}
        </p>
      )}
      <ErrorNote error={grid.error} />
      {grid.isLoading && <SkeletonList rows={4} label="Loading the timesheet grid" />}
      {data && rows.length === 0 && (
        <EmptyState title="No one to show" description="Billable people appear here once they are in People. Timesheets that arrive are added to the right person and day." />
      )}
      {rows.length > 0 && (
        <div className="max-h-[70vh] overflow-auto rounded-sm border border-rule bg-surface" tabIndex={0} aria-label="Timesheet grid">
          <table className="ledger w-max min-w-full text-left text-sm">
            <thead>
              <tr>
                <th scope="col" className="sticky top-0 left-0 z-20 min-w-48 bg-surface">
                  Person
                </th>
                {days.map((d) => (
                  <th key={d.date} scope="col" className={cn('sticky top-0 z-10 min-w-11 px-2! text-center', d.weekend ? 'bg-neutral-tint' : 'bg-surface')}>
                    <span className="block">{d.weekday}</span>
                    <span className="block font-mono">{Number(d.date.slice(8))}</span>
                  </th>
                ))}
                <th scope="col" className="sticky top-0 z-10 bg-surface text-right">
                  Total
                </th>
                <th scope="col" className="sticky top-0 z-10 bg-surface">
                  Status
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r, i) => {
                const key = `${r.person_id ?? r.record_ids[0] ?? 'x'}-${i}`;
                const open = () => r.record_ids.length > 0 && onOpen(r);
                return (
                  <tr key={key}>
                    <td className="sticky left-0 z-10 bg-surface">
                      <button type="button" className="text-left focus-visible:outline-2 focus-visible:outline-accent" onClick={open}>
                        <span className="block font-semibold text-ink">{r.name}</span>
                        <span className="block font-mono text-xs text-ink-muted">{[r.employee_code, r.client].filter(Boolean).join(' · ') || ' '}</span>
                      </button>
                    </td>
                    {days.map((d) => (
                      <td key={d.date} className={cn('px-2! py-2! text-center font-mono text-xs', d.weekend && 'bg-neutral-tint')}>
                        {r.cells[d.date] !== undefined ? (
                          <button type="button" aria-label={`${r.name}, ${shortDate(d.date)}: ${cellLabel(r.cells[d.date])} hours`} className="w-full text-ink hover:underline" onClick={open}>
                            {cellLabel(r.cells[d.date])}
                          </button>
                        ) : null}
                      </td>
                    ))}
                    <td className="text-right font-mono text-ink">{r.record_ids.length > 0 ? cellLabel(shownTotal(r)) : ''}</td>
                    <td>
                      <Badge tone={TONE[r.status]}>{statusText[r.status]}</Badge>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
