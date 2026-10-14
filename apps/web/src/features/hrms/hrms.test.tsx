import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PortalPage } from './PortalPage';
import { TimesheetsPage } from './TimesheetsPage';
import { gridCsv } from './timesheetFormat';
import type { TimesheetGrid } from './types';

type Handler = (init?: RequestInit) => unknown;

function mockApi(routes: Record<string, unknown | Handler>) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    if (url.endsWith('/api/v1/me')) {
      return Promise.resolve(new Response(JSON.stringify({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role: 'admin', created_at: '' }], auth_mode: 'password' })));
    }
    const key = Object.keys(routes).find((k) => url.includes(k));
    const body = key === undefined ? {} : routes[key];
    return Promise.resolve(new Response(JSON.stringify(typeof body === 'function' ? (body as Handler)(init) : body)));
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  return fetchMock;
}

function renderAt(path: string, route: string, element: React.ReactNode) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path={route} element={element} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const access = (level: string) => ({ level, person: null, is_manager: false, report_count: 0, staffing: true, primary_metric: 'submissions' });
const settings = { tz_offset_minutes: 0, manager_after_hours: null, final_after_hours: null, final_emails: [], below_target_hour: 16, weekly_check_hour: 12, hr_source_id: null, client_domains: [], timesheet_grace_days: 3, hours_tolerance: 0.5, sources: [{ id: 'src_hr', kind: 'nango_gmail', mailbox: 'hr@acme.test' }] };

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

const day = (date: string, weekday: string) => ({ date, weekday, weekend: weekday === 'Sat' || weekday === 'Sun' });
const grid: TimesheetGrid = {
  month: '2026-09',
  due_at: '2026-10-04T00:00:00Z',
  summary: { recorded: 1, needs_check: 1, missing: 1 },
  days: [day('2026-09-04', 'Fri'), day('2026-09-05', 'Sat'), day('2026-09-07', 'Mon'), day('2026-09-08', 'Tue')],
  rows: [
    { person_id: 'p1', name: 'Ravi Kumar', employee_code: 'E1024', client: 'Acme', cells: { '2026-09-04': 8, '2026-09-05': 4.5, '2026-09-07': 8 }, total: 20.5, status: 'recorded', record_ids: ['r1'] },
    { person_id: 'p2', name: 'Dev Patel', employee_code: null, client: 'Globex', cells: { '2026-09-08': 6 }, total: 6, status: 'needs_check', record_ids: ['r2'] },
    { person_id: 'p3', name: 'Asha Rao', employee_code: 'E3', client: 'Acme', cells: {}, total: 0, status: 'missing', record_ids: [] },
  ],
};
const record = (over: Record<string, unknown> = {}) => ({
  id: 'r1', person_id: 'p1', person_name: 'Ravi Kumar', name_as_written: 'Ravi Kumar', employee_code: 'E1024', client: 'Acme', period_start: '2026-09-01', period_end: '2026-09-30',
  total_hours: 20.5, status: 'recorded', source: 'attachment', source_label: 'Excel', attachment_filename: 'timesheet-september.xlsx', match_method: 'code', match_confidence: 0.99, needs_person: false,
  issues: [{ code: 'weekend_hours', message: 'Hours on a weekend: 5 Sep (Sat).', severity: 'note' }], replaces_id: null,
  email: { message_id: 'm1', available: true, subject: 'September timesheet', from: 'ravi@client.test', received_at: '2026-10-02T09:00:00Z' },
  created_at: '2026-10-02T09:00:00Z', updated_at: '2026-10-02T09:00:00Z',
  entries: [{ date: '2026-09-05', hours: 4.5, project: 'Portal' }], identity: { name: 'Ravi Kumar', code: 'E1024' },
  ...over,
});

describe('TimesheetsPage', () => {
  it('shows a grid of people by days with weekends shaded, totals and who is missing', async () => {
    mockApi({ '/team/access': access('operator'), '/team/settings': settings, '/staffing/timesheet-grid': grid });
    renderAt('/w/w1/timesheets', '/w/:ws/timesheets', <TimesheetsPage />);
    const ravi = (await screen.findByText('Ravi Kumar')).closest('tr') as HTMLElement;
    expect(within(ravi).getByText('Recorded')).toBeInTheDocument();
    expect(within(ravi).getByText('E1024 · Acme')).toBeInTheDocument();
    expect(within(ravi).getByLabelText('Ravi Kumar, 5 Sep: 4.5 hours').closest('td')).toHaveClass('bg-neutral-tint');
    expect(within(ravi).getByText('20.5')).toBeInTheDocument();
    expect(within(screen.getByText('Dev Patel').closest('tr') as HTMLElement).getByText('Needs check')).toBeInTheDocument();
    expect(within(screen.getByText('Asha Rao').closest('tr') as HTMLElement).getByText('Missing')).toBeInTheDocument();
    expect(screen.getByText(/1 recorded · 1 need a check · 1 missing/)).toBeInTheDocument();
    expect(screen.queryByLabelText('HR / timesheet mailbox')).toBeNull();
  });

  it('narrows the grid by client and search, and shows one week at a time', async () => {
    mockApi({ '/team/access': access('operator'), '/team/settings': settings, '/staffing/timesheet-grid': grid });
    renderAt('/w/w1/timesheets', '/w/:ws/timesheets', <TimesheetsPage />);
    await screen.findByText('Ravi Kumar');
    await userEvent.selectOptions(screen.getByLabelText('Client or project'), 'Globex');
    expect(screen.queryByText('Ravi Kumar')).toBeNull();
    expect(screen.getByText('Dev Patel')).toBeInTheDocument();
    await userEvent.selectOptions(screen.getByLabelText('Client or project'), '');
    await userEvent.type(screen.getByLabelText('Search people'), 'e3');
    expect(screen.queryByText('Dev Patel')).toBeNull();
    expect(screen.getByText('Asha Rao')).toBeInTheDocument();
    await userEvent.clear(screen.getByLabelText('Search people'));
    await userEvent.click(screen.getByRole('button', { name: 'Week' }));
    expect(screen.getByLabelText('Week')).toHaveDisplayValue('4 Sep – 5 Sep');
    expect(screen.queryByLabelText('Dev Patel, 8 Sep: 6 hours')).toBeNull();
    await userEvent.selectOptions(screen.getByLabelText('Week'), '1');
    expect(screen.getByLabelText('Dev Patel, 8 Sep: 6 hours')).toBeInTheDocument();
  });

  it('opens a person\'s record from a cell, with the days, what to check and the email', async () => {
    mockApi({ '/team/access': access('operator'), '/team/settings': settings, '/staffing/timesheet-records/r1': record(), '/staffing/timesheet-grid': grid });
    renderAt('/w/w1/timesheets', '/w/:ws/timesheets', <TimesheetsPage />);
    await userEvent.click(await screen.findByLabelText('Ravi Kumar, 5 Sep: 4.5 hours'));
    const drawer = await screen.findByRole('dialog', { name: "Ravi Kumar's timesheets" });
    expect(await within(drawer).findByText('Hours on a weekend: 5 Sep (Sat).')).toBeInTheDocument();
    expect(within(drawer).getByText('Portal')).toBeInTheDocument();
    expect(within(drawer).getByText(/Excel file \(timesheet-september.xlsx\)/)).toBeInTheDocument();
    expect(within(drawer).getByRole('link', { name: 'Open email' })).toHaveAttribute('href', '/w/w1/inbox/m1');
  });

  it('says so when the email is gone', async () => {
    mockApi({
      '/team/access': access('operator'),
      '/team/settings': settings,
      '/staffing/timesheet-records/r1': record({ email: { message_id: null, available: false, subject: 'September timesheet', from: 'ravi@client.test', received_at: null } }),
      '/staffing/timesheet-grid': grid,
    });
    renderAt('/w/w1/timesheets?record=r1', '/w/:ws/timesheets', <TimesheetsPage />);
    expect(await screen.findByText('Email removed from Post Room')).toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Open email' })).toBeNull();
  });

  it('lists records with search and lets the super admin link an unmatched one to a person', async () => {
    const fetchMock = mockApi({
      '/team/access': access('super_admin'),
      '/team/settings': settings,
      '/team/people': { items: [{ id: 'p1', name: 'Ravi Kumar', emails: ['ravi@client.test'], manager_id: null, role: null, joining_date: null, daily_target: null, weekly_target: null, source_id: null, active: true }], can_edit: true },
      '/staffing/timesheet-records/r9/link': record({ id: 'r9', needs_person: false }),
      '/staffing/timesheet-records/r9': record({ id: 'r9', person_id: null, person_name: null, name_as_written: 'D. Rao', needs_person: true, status: 'needs_check', issues: [{ code: 'person_unmatched', message: 'Who is this? (D. Rao)', severity: 'check' }] }),
      '/staffing/timesheet-records': { items: [record({ id: 'r9', person_id: null, person_name: null, name_as_written: 'D. Rao', needs_person: true, status: 'needs_check', source: 'pasted', source_label: 'Pasted' })] },
      '/staffing/timesheet-grid': grid,
    });
    renderAt('/w/w1/timesheets', '/w/:ws/timesheets', <TimesheetsPage />);
    await userEvent.click(await screen.findByRole('tab', { name: 'Records' }));
    const row = (await screen.findByText('D. Rao')).closest('tr') as HTMLElement;
    expect(within(row).getByText('Needs check')).toBeInTheDocument();
    expect(within(row).getByText('Pasted')).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText('Search records'), 'rao');
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).includes('q=rao'))).toBe(true));
    await userEvent.click(within((await screen.findByText('D. Rao')).closest('tr') as HTMLElement).getByRole('button', { name: 'Details' }));
    expect((await screen.findAllByText('Who is this? (D. Rao)')).length).toBeGreaterThan(0);
    await userEvent.selectOptions(await screen.findByLabelText('Person to link'), 'p1');
    await userEvent.click(screen.getByRole('button', { name: 'Link to person' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u, init]) => String(u).endsWith('/r9/link') && init?.method === 'POST')).toBe(true));
    const post = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
    expect(JSON.parse(post?.[1]?.body as string)).toEqual({ person_id: 'p1' });
  });

  it('exports the grid as CSV', () => {
    const csv = gridCsv(grid.days, grid.rows, (r) => r.total);
    const lines = csv.split('\n');
    expect(lines[0]).toBe('Person,Employee ID,Client,2026-09-04,2026-09-05,2026-09-07,2026-09-08,Total hours,Status');
    expect(lines[1]).toBe('Ravi Kumar,E1024,Acme,8,4.5,8,,20.5,Recorded');
    expect(lines[3]).toBe('Asha Rao,E3,Acme,,,,,0,Missing');
  });

  it('lets the super admin name the HR mailbox', async () => {
    const fetchMock = mockApi({
      '/team/access': access('super_admin'),
      '/team/settings': (init?: RequestInit) => (init?.body ? JSON.parse(init.body as string) : settings),
      '/staffing/timesheet-grid': { ...grid, rows: [], days: [], summary: { recorded: 0, needs_check: 0, missing: 0 } },
    });
    renderAt('/w/w1/timesheets', '/w/:ws/timesheets', <TimesheetsPage />);
    expect(await screen.findByText('No one to show')).toBeInTheDocument();
    await userEvent.selectOptions(await screen.findByLabelText('HR / timesheet mailbox'), 'src_hr');
    await userEvent.click(screen.getByRole('button', { name: 'Save policy' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([, init]) => init?.method === 'PUT')).toBe(true));
    const put = fetchMock.mock.calls.find(([, init]) => init?.method === 'PUT');
    expect(JSON.parse(put?.[1]?.body as string)).toMatchObject({ hr_source_id: 'src_hr', timesheet_grace_days: 3 });
  });
});

describe('PortalPage', () => {
  it('guides a new super admin to import the first export and reports bad rows', async () => {
    const report = { dry_run: true, total_rows: 2, created: 1, updated: 0, skipped: 1, errors: [{ row: 3, field: 'email', message: "no one in the directory has the email 'x@y.test'" }] };
    const fetchMock = mockApi({
      '/team/access': access('super_admin'),
      '/staffing/portal/mismatches': { items: [], portal_hours_rows: 0, portal_submission_rows: 0 },
      '/staffing/portal/hours': report,
    });
    renderAt('/w/w1/portal', '/w/:ws/portal', <PortalPage />);
    expect(await screen.findByText('No portal data yet')).toBeInTheDocument();
    await userEvent.type(screen.getByLabelText('Portal hours CSV'), 'email,month,hours{enter}x@y.test,2026-09,160');
    const section = screen.getByRole('region', { name: 'Import portal hours' });
    await userEvent.click(within(section).getByRole('button', { name: 'Check only' }));
    expect(await within(section).findByRole('status')).toHaveTextContent("Row 3, email: no one in the directory has the email 'x@y.test'");
    expect(JSON.parse(fetchMock.mock.calls.find(([url]) => String(url).endsWith('/portal/hours'))?.[1]?.body as string)).toMatchObject({ dry_run: true });
  });

  it('lists open mismatches for everyone but offers imports only to the super admin', async () => {
    mockApi({
      '/team/access': access('operator'),
      '/staffing/portal/mismatches': {
        portal_hours_rows: 2, portal_submission_rows: 0,
        items: [{ id: 'x1', person_id: 'p1', person_name: 'Ravi Kumar', month: '2026-09', kind: 'hours_differ', detail: { portal_hours: 160, email_hours: 152 }, status: 'open', updated_at: '2026-10-05T00:00:00Z' }],
      },
    });
    renderAt('/w/w1/portal', '/w/:ws/portal', <PortalPage />);
    const row = (await screen.findByText('Ravi Kumar')).closest('tr') as HTMLElement;
    expect(within(row).getByText('Hours differ')).toBeInTheDocument();
    expect(within(row).getByText('Portal 160 h · Mail 152 h')).toBeInTheDocument();
    expect(screen.queryByLabelText('Portal hours CSV')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Compare now' })).toBeNull();
  });
});
