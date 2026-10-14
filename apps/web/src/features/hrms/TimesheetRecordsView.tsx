import { Fragment, useState } from 'react';
import { Badge, Button, EmptyState, Input, Select, SkeletonList, Table, Td, Th } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useDebounced } from '../../lib/useDebounced';
import { useTimesheetRecords } from './hooks';
import { RECORD_LABEL, RECORD_TONE, RecordById } from './RecordDetail';
import { currentMonth, hoursLabel, periodLabel } from './timesheetFormat';

/** Every timesheet that was read, newest first, with search and filters. */
export function TimesheetRecordsView({ ws, canLink }: { ws: string; canLink: boolean }) {
  const [q, setQ] = useState('');
  const [month, setMonth] = useState('');
  const [status, setStatus] = useState('');
  const [open, setOpen] = useState<string>();
  const search = useDebounced(q, 250);
  const records = useTimesheetRecords(ws, { q: search || undefined, month: month || undefined, status: status || undefined });
  const items = records.data?.items ?? [];
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end gap-3">
        <Input aria-label="Search records" className="max-w-64" placeholder="Search name, ID, client, file" value={q} onChange={(e) => setQ(e.target.value)} />
        <Input aria-label="Month" type="month" className="w-44" value={month} max={currentMonth()} onChange={(e) => setMonth(e.target.value)} />
        <Select aria-label="Status" className="w-44" value={status} onChange={(e) => setStatus(e.target.value)}>
          <option value="">Any status</option>
          <option value="recorded">Recorded</option>
          <option value="needs_check">Needs check</option>
          <option value="replaced">Replaced</option>
        </Select>
      </div>
      <ErrorNote error={records.error} />
      {records.isLoading && <SkeletonList rows={3} label="Loading records" />}
      {records.data && items.length === 0 && <EmptyState title="No timesheets recorded" description="When a timesheet arrives, pasted in an email or attached, it is read and kept here." />}
      {items.length > 0 && (
        <Table>
          <thead>
            <tr>
              <Th>Person</Th>
              <Th>Period</Th>
              <Th>Total</Th>
              <Th>Status</Th>
              <Th>Source</Th>
              <Th>
                <span className="sr-only">Details</span>
              </Th>
            </tr>
          </thead>
          <tbody>
            {items.map((r) => (
              <Fragment key={r.id}>
                <tr>
                  <Td className="font-semibold text-ink">{r.person_name ?? r.name_as_written ?? 'Not matched'}</Td>
                  <Td>{periodLabel(r.period_start, r.period_end)}</Td>
                  <Td>{hoursLabel(r.total_hours)}</Td>
                  <Td>
                    <Badge tone={RECORD_TONE[r.status]}>{RECORD_LABEL[r.status]}</Badge>
                  </Td>
                  <Td>{r.source_label}</Td>
                  <Td>
                    <Button variant="ghost" size="sm" aria-expanded={open === r.id} onClick={() => setOpen(open === r.id ? undefined : r.id)}>
                      {open === r.id ? 'Hide' : 'Details'}
                    </Button>
                  </Td>
                </tr>
                {open === r.id && (
                  <tr>
                    <Td colSpan={6} className="bg-surface">
                      <RecordById ws={ws} id={r.id} canLink={canLink} />
                    </Td>
                  </tr>
                )}
              </Fragment>
            ))}
          </tbody>
        </Table>
      )}
    </div>
  );
}
