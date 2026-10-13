import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { EscalationsPage } from './EscalationsPage';
import { PeoplePage } from './PeoplePage';
import { PerformancePage } from './PerformancePage';
import { PerformanceWidget } from './PerformanceWidget';
import type { MetricValue, PersonMetrics, TeamAccess } from './types';

const metric = (over: Partial<MetricValue>): MetricValue => ({ key: 'submissions', label: 'Submissions', unit: 'count', primary: true, today: 0, week: 0, since_joining: 0, ...over });

const priya: PersonMetrics = {
  person_id: 'p1',
  name: 'Priya Sharma',
  role: 'recruiter',
  joining_date: '2026-03-02',
  primary_metric: 'submissions',
  daily_target: 5,
  weekly_target: 25,
  today: 3,
  week: 14,
  metrics: [metric({ today: 3, week: 14, since_joining: 90 }), metric({ key: 'interview_rate', label: 'Interview rate', unit: 'rate', primary: false, today: null, week: 0.3, since_joining: 0.32 })],
};

const access = (over: Partial<TeamAccess> = {}): TeamAccess => ({ level: 'super_admin', person: null, is_manager: false, report_count: 0, staffing: true, primary_metric: 'submissions', ...over });

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
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path={route} element={element} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe('PerformanceWidget', () => {
  it('shows today and week against target and the quality number for someone in the directory', async () => {
    mockApi({ '/team/metrics/me': { person: { id: 'p1', name: 'Priya Sharma' }, metrics: priya } });
    renderAt('/w/w1/inbox', '/w/:ws/inbox', <PerformanceWidget ws="w1" />);
    const link = await screen.findByRole('link', { name: /Your performance/ });
    expect(link).toHaveAttribute('href', '/w/w1/performance');
    expect(link).toHaveTextContent('Today 3/5 · Week 14/25 · Interview rate 32%');
  });

  it('works with the generic replies-sent metric in other packs', async () => {
    const generic: PersonMetrics = {
      ...priya,
      primary_metric: 'replies_sent',
      daily_target: 20,
      weekly_target: null,
      today: 12,
      week: 41,
      metrics: [metric({ key: 'replies_sent', label: 'Replies sent' }), metric({ key: 'avg_response_minutes', label: 'Average response time', unit: 'minutes', primary: false, since_joining: 45 })],
    };
    mockApi({ '/team/metrics/me': { person: { id: 'p1', name: 'Asha' }, metrics: generic } });
    renderAt('/w/w1/inbox', '/w/:ws/inbox', <PerformanceWidget ws="w1" />);
    expect(await screen.findByRole('link', { name: /Your performance/ })).toHaveTextContent('Today 12/20 · Week 41 · Average response time 45 min');
  });

  it('renders nothing for someone who is not in the directory', async () => {
    const fetchMock = mockApi({ '/team/metrics/me': { person: null, metrics: null } });
    const { container } = renderAt('/w/w1/inbox', '/w/:ws/inbox', <PerformanceWidget ws="w1" />);
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });
});

describe('PerformancePage', () => {
  const page = () => renderAt('/w/w1/performance', '/w/:ws/performance', <PerformancePage />);

  it('lists the team against targets and shows detected submissions that need confirming', async () => {
    const fetchMock = mockApi({
      '/team/access': access(),
      '/team/metrics/me': { person: null, metrics: null },
      '/team/metrics/team': { manager_id: null, primary_metric: 'submissions', today: 3, week: 14, daily_target: 5, weekly_target: 25, metrics: priya.metrics, members: [priya] },
      '/staffing/submissions': {
        needs_confirm: 1,
        items: [
          {
            id: 's1', person_id: 'p1', person_name: 'Priya Sharma', sent_at: new Date().toISOString(), client_domain: 'capgemini.com', client: null,
            attachment_name: 'Anita_CV.pdf', candidate_name: 'Anita Desai', candidate_id: null, requirement_id: null, requirement_title: null,
            link_status: 'needs_confirm', options: { requirements: [{ id: 'r1', role_title: 'Java Developer', client: 'Capgemini', score: 0.5 }, { id: 'r2', role_title: 'Data Engineer', client: 'Capgemini', score: 0.5 }] },
            status: 'active', outcome: 'pending', reached_interview: false,
          },
        ],
      },
    });
    page();
    expect(await screen.findByRole('heading', { name: 'Team performance' })).toBeInTheDocument();
    const row = (await screen.findByText('Priya Sharma', { selector: 'p' })).closest('tr') as HTMLElement;
    expect(within(row).getByText('3/5')).toBeInTheDocument();
    expect(within(row).getByText('14/25')).toBeInTheDocument();
    expect(within(row).getByText('32%')).toBeInTheDocument();

    await userEvent.click(await screen.findByRole('button', { name: /Data Engineer/ }));
    const patch = fetchMock.mock.calls.find(([, init]) => init?.method === 'PATCH');
    expect(patch?.[0]).toBe('/api/v1/workspaces/w1/staffing/submissions/s1');
    expect(JSON.parse(patch?.[1]?.body as string)).toEqual({ requirement_id: 'r2' });
  });

  it('tells a new super admin to add people first', async () => {
    mockApi({
      '/team/access': access({ staffing: false, primary_metric: 'replies_sent' }),
      '/team/metrics/me': { person: null, metrics: null },
      '/team/metrics/team': { manager_id: null, primary_metric: 'replies_sent', today: 0, week: 0, daily_target: 0, weekly_target: 0, metrics: [], members: [] },
    });
    page();
    expect(await screen.findByText('No one to measure yet')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Go to People' })).toHaveAttribute('href', '/w/w1/people');
    expect(screen.queryByText('Submissions')).toBeNull();
  });

  it('explains an empty team to someone with no reports and shows their own numbers', async () => {
    mockApi({
      '/team/access': access({ level: 'operator', staffing: false }),
      '/team/metrics/me': { person: { id: 'p1', name: 'Priya' }, metrics: priya },
      '/team/metrics/team': { manager_id: 'p1', primary_metric: 'submissions', today: 0, week: 0, daily_target: 0, weekly_target: 0, metrics: [], members: [] },
    });
    page();
    expect(await screen.findByText('No team to show')).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Your numbers' })).toHaveTextContent('3/5');
  });
});

describe('PeoplePage', () => {
  const page = () => renderAt('/w/w1/people', '/w/:ws/people', <PeoplePage />);
  const person = { id: 'p1', name: 'Priya Sharma', emails: ['priya@acme.test'], employee_type: 'non_billable', client: null, manager_id: null, role: 'recruiter', joining_date: '2026-03-02', daily_target: 5, weekly_target: 25, source_id: null, active: true };

  it('shows a first-step empty state and lets the super admin add someone', async () => {
    const fetchMock = mockApi({ '/team/access': access(), '/team/people': (init?: RequestInit) => (init?.method === 'POST' ? person : { items: [], can_edit: true }) });
    page();
    expect(await screen.findByText('No one in the directory yet')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Add the first person' }));
    const dialog = await screen.findByRole('dialog', { name: 'Add a person' });
    await userEvent.type(within(dialog).getByLabelText('Name'), 'Priya Sharma');
    await userEvent.type(within(dialog).getByLabelText(/Email addresses/), 'priya@acme.test, priya.s@acme.test');
    await userEvent.type(within(dialog).getByLabelText('Daily target'), '5');
    await userEvent.selectOptions(within(dialog).getByLabelText('Employee type'), 'billable');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Add person' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([, init]) => init?.method === 'POST')).toBe(true));
    const post = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
    expect(post?.[0]).toBe('/api/v1/workspaces/w1/team/people');
    expect(JSON.parse(post?.[1]?.body as string)).toMatchObject({ name: 'Priya Sharma', emails: ['priya@acme.test', 'priya.s@acme.test'], daily_target: 5, employee_type: 'billable', manager_id: null });
  });

  it('is read-only for everyone but the super admin, and hides HR fields outside the pack', async () => {
    mockApi({ '/team/access': access({ level: 'operator', staffing: false }), '/team/people': { items: [{ ...person, employee_type: undefined }], can_edit: false } });
    page();
    expect(await screen.findByText('Priya Sharma')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Add person' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Edit Priya Sharma' })).toBeNull();
    expect(screen.queryByText('Billable')).toBeNull();
    expect(screen.getByText(/Only the workspace's super admin/)).toBeInTheDocument();
  });

  it('checks a CSV without saving and lists each problem row', async () => {
    const report = { dry_run: true, total_rows: 3, created: 1, updated: 0, skipped: 2, errors: [{ row: 3, field: 'emails', message: "'x' is not a valid email address" }, { row: 4, field: 'joining_date', message: 'use YYYY-MM-DD' }] };
    const fetchMock = mockApi({ '/team/access': access(), '/team/people/import': report, '/team/people': { items: [person], can_edit: true } });
    page();
    await userEvent.click(await screen.findByRole('button', { name: 'Import CSV' }));
    await userEvent.type(screen.getByLabelText('People CSV'), 'name,email{enter}A,x');
    await userEvent.click(screen.getByRole('button', { name: 'Check only' }));
    const status = await screen.findByRole('status');
    expect(status).toHaveTextContent('Checked, nothing saved: 1 new row, 0 updated rows, 2 skipped rows.');
    expect(status).toHaveTextContent("Row 3, emails: 'x' is not a valid email address");
    expect(status).toHaveTextContent('Row 4, joining_date: use YYYY-MM-DD');
    const post = fetchMock.mock.calls.find(([url]) => String(url).endsWith('/people/import'));
    expect(JSON.parse(post?.[1]?.body as string)).toMatchObject({ dry_run: true });
  });
});

describe('EscalationsPage', () => {
  it('shows who is told at each level and lets the super admin change the delays', async () => {
    const fetchMock = mockApi({
      '/team/access': access(),
      '/team/escalations': {
        manager_after_hours: 24, final_after_hours: 24, final_emails: ['hr@acme.test'], can_edit: true,
        chains: [{ person: { id: 'p1', name: 'Priya Sharma', role: 'recruiter' }, steps: [
          { level: 1, label: 'Owner', name: 'Priya Sharma', emails: ['priya@acme.test'] },
          { level: 2, label: 'Manager', name: 'Neha Rao', emails: ['neha@acme.test'] },
          { level: 3, label: 'Final contacts', name: null, emails: ['hr@acme.test'] },
        ] }],
      },
      '/team/settings': (init?: RequestInit) => ({ tz_offset_minutes: 0, manager_after_hours: null, final_after_hours: null, final_emails: ['hr@acme.test'], below_target_hour: 16, weekly_check_hour: 12, ...(init?.body ? JSON.parse(init.body as string) : {}) }),
    });
    renderAt('/w/w1/escalations', '/w/:ws/escalations', <EscalationsPage />);
    const chain = (await screen.findByText('Priya Sharma', { selector: 'p' })).closest('li') as HTMLElement;
    expect(chain).toHaveTextContent('1. Owner: priya@acme.test');
    expect(chain).toHaveTextContent('2. Manager: neha@acme.test');
    expect(chain).toHaveTextContent('3. Final contacts: hr@acme.test');

    const hours = await screen.findByLabelText('Hours with the owner before the manager is told');
    await userEvent.clear(hours);
    await userEvent.type(hours, '6');
    await userEvent.click(screen.getByRole('button', { name: 'Save' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([, init]) => init?.method === 'PUT')).toBe(true));
    const put = fetchMock.mock.calls.find(([, init]) => init?.method === 'PUT');
    expect(JSON.parse(put?.[1]?.body as string)).toMatchObject({ manager_after_hours: 6, final_after_hours: 24, final_emails: ['hr@acme.test'] });
  });

  it('explains what to do first when there is nobody yet', async () => {
    mockApi({ '/team/access': access({ level: 'operator', staffing: false }), '/team/escalations': { manager_after_hours: 24, final_after_hours: 24, final_emails: [], can_edit: false, chains: [] }, '/team/settings': {} });
    renderAt('/w/w1/escalations', '/w/:ws/escalations', <EscalationsPage />);
    expect(await screen.findByText('No chains to show yet')).toBeInTheDocument();
    expect(screen.getByText(/Only the super admin changes this/)).toBeInTheDocument();
  });
});
