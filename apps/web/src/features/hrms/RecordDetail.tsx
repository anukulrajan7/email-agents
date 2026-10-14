import { useState } from 'react';
import { Badge, Button, Select, SkeletonList, TextLink, type Tone } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { fullDate } from '../../lib/format';
import { usePeople } from '../team/hooks';
import { useLinkTimesheetRecord, useTimesheetRecord } from './hooks';
import { cellLabel, hoursLabel, periodLabel, shortDate } from './timesheetFormat';
import type { RecordStatus, TimesheetRecord } from './types';

export const RECORD_TONE: Record<RecordStatus, Tone> = { recorded: 'success', replaced: 'neutral', needs_check: 'warning' };
export const RECORD_LABEL: Record<RecordStatus, string> = { recorded: 'Recorded', replaced: 'Replaced', needs_check: 'Needs check' };

function LinkToPerson({ ws, record }: { ws: string; record: TimesheetRecord }) {
  const people = usePeople(ws);
  const link = useLinkTimesheetRecord(ws);
  const [person, setPerson] = useState('');
  return (
    <div className="flex flex-col gap-2 rounded-sm border border-review/50 bg-review-tint p-3">
      <p className="text-sm text-ink">
        Who is this? {record.name_as_written ?? 'No name'}
        {record.employee_code ? ` (ID ${record.employee_code})` : ''} is not matched to anyone in People. Link it once and the next sheet with the same name or ID is matched on its own.
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Select aria-label="Person to link" className="max-w-64" value={person} onChange={(e) => setPerson(e.target.value)}>
          <option value="">Choose a person</option>
          {(people.data?.items ?? []).map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </Select>
        <Button size="sm" disabled={!person} loading={link.isPending} onClick={() => link.mutate({ id: record.id, person_id: person })}>
          Link to person
        </Button>
      </div>
      <ErrorNote error={link.error} />
    </div>
  );
}

/** One record: who and when, the days, what to check, where it came from. */
export function RecordDetail({ ws, record, canLink }: { ws: string; record: TimesheetRecord; canLink: boolean }) {
  const entries = record.entries ?? [];
  const identity = record.identity ?? {};
  const written = [
    ['Name', identity.name],
    ['Employee ID', identity.code],
    ['Email', identity.email],
    ['Client', identity.client],
    ['Manager', identity.manager],
    ['Location', identity.location],
  ].filter((row): row is [string, string] => Boolean(row[1]));
  return (
    <div className="flex flex-col gap-4 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <Badge tone={RECORD_TONE[record.status]}>{RECORD_LABEL[record.status]}</Badge>
        <span className="font-semibold text-ink">{record.person_name ?? record.name_as_written ?? 'Not matched to a person'}</span>
        <span className="text-ink-muted">
          {periodLabel(record.period_start, record.period_end)} · {hoursLabel(record.total_hours)}
        </span>
      </div>
      {record.needs_person && canLink && <LinkToPerson ws={ws} record={record} />}
      {record.issues.length > 0 && (
        <ul className="flex flex-col gap-1" aria-label="Things to check">
          {record.issues.map((i) => (
            <li key={i.code} className={i.severity === 'check' ? 'text-danger' : 'text-ink-muted'}>
              {i.message}
            </li>
          ))}
        </ul>
      )}
      {entries.length > 0 && (
        <div className="max-h-72 overflow-auto rounded-sm border border-rule">
          <table className="ledger w-full text-left text-sm">
            <thead>
              <tr>
                <th scope="col">Day</th>
                <th scope="col">Hours</th>
                <th scope="col">Project</th>
                <th scope="col">Note</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((e, i) => (
                <tr key={`${e.date}-${i}`}>
                  <td>{e.date ? shortDate(e.date) : '–'}</td>
                  <td>{cellLabel(e.hours)}</td>
                  <td>{e.project ?? '–'}</td>
                  <td>{e.note ?? '–'}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {written.length > 0 && (
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
          {written.map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-ink-muted">{k} as written</dt>
              <dd className="text-ink">{v}</dd>
            </div>
          ))}
        </dl>
      )}
      <div className="flex flex-col gap-1 text-ink-muted">
        <span>
          Source: {record.source === 'pasted' ? 'Pasted in the email' : `${record.source_label} file${record.attachment_filename ? ` (${record.attachment_filename})` : ''}`}
        </span>
        {record.email.subject && (
          <span>
            Email: {record.email.subject}
            {record.email.from ? `, from ${record.email.from}` : ''}
            {record.email.received_at ? `, ${fullDate(record.email.received_at)}` : ''}
          </span>
        )}
        {record.email.available && record.email.message_id ? (
          <TextLink className="self-start" to={`/w/${ws}/inbox/${record.email.message_id}`}>
            Open email
          </TextLink>
        ) : (
          <span>Email removed from Post Room</span>
        )}
      </div>
    </div>
  );
}

/** A record loaded by id, for places that only know the id (a grid row, a link from the reader). */
export function RecordById({ ws, id, canLink }: { ws: string; id: string; canLink: boolean }) {
  const record = useTimesheetRecord(ws, id);
  return (
    <>
      <ErrorNote error={record.error} />
      {record.data ? <RecordDetail ws={ws} record={record.data} canLink={canLink} /> : <SkeletonList rows={2} label="Loading the timesheet" />}
    </>
  );
}
