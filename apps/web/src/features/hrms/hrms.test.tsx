import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PortalPage } from './PortalPage';
import { TimesheetsPage } from './TimesheetsPage';

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

describe('TimesheetsPage', () => {
  it('shows each billable person as received, not due yet or missing', async () => {
    mockApi({
      '/team/access': access('operator'),
      '/team/settings': settings,
      '/staffing/timesheets': {
        month: '2026-09', grace_days: 3, hr_mailbox_set: true, missing: 1,
        items: [
          { person_id: 'p1', name: 'Ravi Kumar', month: '2026-09', status: 'missing', due_at: '2026-10-04T00:00:00Z', timesheet: null },
          { person_id: 'p2', name: 'Dev Patel', month: '2026-09', status: 'received', due_at: '2026-10-04T00:00:00Z', timesheet: { message_id: 'm1', subject: 'Sept', received_at: '2026-09-30T10:00:00Z', hours: 168 } },
        ],
      },
    });
    renderAt('/w/w1/timesheets', '/w/:ws/timesheets', <TimesheetsPage />);
    const ravi = (await screen.findByText('Ravi Kumar')).closest('tr') as HTMLElement;
    expect(within(ravi).getByText('Missing')).toBeInTheDocument();
    const dev = screen.getByText('Dev Patel').closest('tr') as HTMLElement;
    expect(within(dev).getByText('Received')).toBeInTheDocument();
    expect(within(dev).getByText('168')).toBeInTheDocument();
    expect(screen.getByText(/1 missing/)).toBeInTheDocument();
    expect(screen.queryByLabelText('HR / timesheet mailbox')).toBeNull();
  });

  it('lets the super admin name the HR mailbox and tells a new user to mark people billable first', async () => {
    const fetchMock = mockApi({
      '/team/access': access('super_admin'),
      '/team/settings': (init?: RequestInit) => (init?.body ? JSON.parse(init.body as string) : settings),
      '/staffing/timesheets': { month: '2026-09', grace_days: 3, hr_mailbox_set: false, missing: 0, items: [] },
    });
    renderAt('/w/w1/timesheets', '/w/:ws/timesheets', <TimesheetsPage />);
    expect(await screen.findByText('No billable people yet')).toBeInTheDocument();
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
