import type { GridDay, GridRow } from './types';

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

export function currentMonth(now = new Date()): string {
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, '0')}`;
}

const parts = (date: string) => ({ y: date.slice(0, 4), m: Number(date.slice(5, 7)), d: Number(date.slice(8, 10)) });

export function shortDate(date: string): string {
  const { m, d } = parts(date);
  return `${d} ${MONTHS[m - 1] ?? ''}`;
}

/** "Sep 2026" for a whole month, else "1 Sep – 12 Sep 2026". */
export function periodLabel(start: string, end: string): string {
  const a = parts(start);
  const b = parts(end);
  const lastDay = new Date(Date.UTC(Number(b.y), b.m, 0)).getUTCDate();
  if (a.y === b.y && a.m === b.m && a.d === 1 && b.d === lastDay) return `${MONTHS[a.m - 1]} ${a.y}`;
  return `${shortDate(start)} – ${shortDate(end)} ${b.y}`;
}

export const hoursLabel = (hours: number | null | undefined): string => (hours === null || hours === undefined ? '–' : `${Number(hours.toFixed(2))} h`);

export const cellLabel = (hours: number | undefined): string => (hours === undefined ? '' : String(Number(hours.toFixed(2))));

/** Days split at Mondays, so a week never spans two rows of the grid. */
export function weeksOf(days: GridDay[]): GridDay[][] {
  const weeks: GridDay[][] = [];
  for (const day of days) {
    if (day.weekday === 'Mon' || weeks.length === 0) weeks.push([]);
    weeks[weeks.length - 1]?.push(day);
  }
  return weeks;
}

const STATUS_TEXT = { recorded: 'Recorded', needs_check: 'Needs check', missing: 'Missing', pending: 'Not due yet' } as const;

/** A cell that starts like a formula is read as text by Excel and Sheets. */
const safe = (v: string) => (/^[=+\-@\t\r]/.test(v) ? `'${v}` : v);

const quote = (v: string) => {
  const s = safe(v);
  return /[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s;
};

/** The grid as CSV: one line per person, the shown days, total and status. */
export function gridCsv(days: GridDay[], rows: GridRow[], total: (row: GridRow) => number): string {
  const head = ['Person', 'Employee ID', 'Client', ...days.map((d) => d.date), 'Total hours', 'Status'];
  const lines = rows.map((r) => [r.name, r.employee_code ?? '', r.client ?? '', ...days.map((d) => cellLabel(r.cells[d.date])), cellLabel(total(r)), STATUS_TEXT[r.status]]);
  return [head, ...lines].map((line) => line.map((c) => quote(String(c))).join(',')).join('\n');
}

export const statusText = STATUS_TEXT;
