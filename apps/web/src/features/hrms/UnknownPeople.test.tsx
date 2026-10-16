import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ToastProvider } from '../../components/ui';
import { TimesheetsPage } from './TimesheetsPage';
import type { IdentityList } from './types';

function mockApi(routes: Record<string, unknown>) {
  const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
    if (url.endsWith('/api/v1/me')) {
      return Promise.resolve(new Response(JSON.stringify({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role: 'admin', created_at: '' }], auth_mode: 'password' })));
    }
    const key = Object.keys(routes).find((k) => url.includes(k));
    return Promise.resolve(new Response(JSON.stringify(key === undefined ? {} : routes[key])));
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  return fetchMock;
}

function renderPage() {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ToastProvider>
        <MemoryRouter initialEntries={['/w/w1/timesheets']}>
          <Routes>
            <Route path="/w/:ws/timesheets" element={<TimesheetsPage />} />
          </Routes>
        </MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>,
  );
}

const access = (level: string) => ({ level, person: null, is_manager: false, report_count: 0, staffing: true, primary_metric: 'submissions' });
const settings = { tz_offset_minutes: 0, manager_after_hours: null, final_after_hours: null, final_emails: [], below_target_hour: 16, weekly_check_hour: 12, hr_source_id: null, client_domains: [], timesheet_grace_days: 3, hours_tolerance: 0.5, sources: [] };
const people = { items: [{ id: 'p1', name: 'Rahul Verma', emails: ['rahul@client.test'], manager_id: null, role: null, joining_date: null, daily_target: null, weekly_target: null, source_id: null, active: true }], can_edit: true };
const grid = { month: '2026-09', due_at: '2026-10-04T00:00:00Z', summary: { recorded: 0, needs_check: 0, missing: 0, unresolved: 1 }, days: [], rows: [] };

const unknown: IdentityList = {
  enabled: true,
  items: [
    {
      id: 'idn1',
      name: 'Rahul Sharma',
      employee_code: 'X-9',
      sender: 'hr@client.test',
      sender_domain: 'client.test',
      record_count: 2,
      records: [
        { id: 'r1', period_start: '2026-09-01', period_end: '2026-09-30', total_hours: 23.5, file: 'rahul.xlsx', subject: 'Timesheets', received_at: null, message_id: 'm1', possible_duplicate: false },
        { id: 'r2', period_start: '2026-09-01', period_end: '2026-09-30', total_hours: 23.5, file: 'rahul-again.xlsx', subject: 'Timesheets', received_at: null, message_id: 'm2', possible_duplicate: true },
      ],
      suggestions: [{ person_id: 'p1', name: 'Rahul Verma', reason: 'Same first name', weak: true }],
      updated_at: '2026-10-02T09:00:00Z',
    },
  ],
  recent: [
    { id: 'idc1', kind: 'link', person_id: 'p1', person_name: 'Rahul Verma', name: 'Rahul Sharma', record_count: 1, by: 'user', undone: false, can_undo: true, at: '2026-10-02T09:00:00Z' },
    { id: 'idc2', kind: 'link', person_id: 'p2', person_name: 'Dinesh Rao', name: 'Dinesh Rao', record_count: 2, added_person: true, by: 'user', undone: false, can_undo: true, at: '2026-10-02T09:01:00Z' },
    { id: 'idc3', kind: 'mark_external', person_id: null, person_name: null, name: 'Client Staff', record_count: 0, by: 'user', undone: false, can_undo: true, at: '2026-10-02T09:02:00Z' },
  ],
};

const base = { '/team/access': access('super_admin'), '/team/settings': settings, '/team/people': people, '/staffing/timesheet-grid': grid };

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

const posts = (fetchMock: ReturnType<typeof mockApi>) => fetchMock.mock.calls.filter(([, init]) => init?.method === 'POST');
const bodyOf = (call: [string, RequestInit?] | undefined) => JSON.parse((call?.[1]?.body as string | undefined) ?? 'null');

describe('People we don\'t know yet', () => {
  it('shows each name as written once, with its sheets and who it might be, and links nothing by itself', async () => {
    const fetchMock = mockApi({ '/staffing/identities': unknown, ...base });
    renderPage();
    const list = await screen.findByRole('list', { name: 'Names to sort out' });
    const card = within(list).getByRole('listitem', { name: 'Rahul Sharma' });
    expect(within(card).getByText('ID X-9')).toBeInTheDocument();
    expect(within(card).getByText(/2 sheets, sent from hr@client.test/)).toBeInTheDocument();
    expect(within(card).getByText('Possible duplicate')).toBeInTheDocument();
    expect(within(card).getByText('Same first name')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: "People we don't know yet" })).toBeInTheDocument();
    expect(posts(fetchMock)).toHaveLength(0);
  });

  it('asks before linking several sheets from a weak suggestion, then links without remembering the name', async () => {
    const fetchMock = mockApi({ '/staffing/identities/idn1/link': {}, '/staffing/identities': unknown, ...base });
    renderPage();
    const suggestion = await screen.findByRole('button', { name: "This is Rahul Verma: link Rahul Sharma's 2 sheets" });
    expect(suggestion).toHaveTextContent('This is Rahul Verma');
    await userEvent.click(suggestion);
    expect(posts(fetchMock)).toHaveLength(0);
    const confirm = await screen.findByRole('dialog', { name: 'Link 2 sheets to Rahul Verma?' });
    expect(within(confirm).getByText(/All 2 sheets written as Rahul Sharma will become Rahul Verma's/)).toBeInTheDocument();
    await userEvent.click(within(confirm).getByRole('button', { name: 'Link 2 sheets' }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(String(posts(fetchMock)[0]?.[0])).toContain('/staffing/identities/idn1/link');
    expect(bodyOf(posts(fetchMock)[0])).toEqual({ person_id: 'p1', remember_name: false });
    expect(await screen.findByText('Rahul Sharma is now Rahul Verma.')).toBeInTheDocument();
  });

  it('links a single sheet from a strong suggestion at once', async () => {
    const one: IdentityList = {
      ...unknown,
      items: [{ ...unknown.items[0]!, record_count: 1, records: unknown.items[0]!.records.slice(0, 1), suggestions: [{ person_id: 'p1', name: 'Rahul Verma', reason: 'Spelled almost the same', weak: false }] }],
    };
    const fetchMock = mockApi({ '/staffing/identities/idn1/link': {}, '/staffing/identities': one, ...base });
    renderPage();
    await userEvent.click(await screen.findByRole('button', { name: "This is Rahul Verma: link Rahul Sharma's 1 sheet" }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(screen.queryByRole('dialog')).toBeNull();
  });

  it('links to someone else in People and can remember the name when the admin ticks the box', async () => {
    const fetchMock = mockApi({ '/staffing/identities/idn1/link': {}, '/staffing/identities': unknown, ...base });
    renderPage();
    await screen.findByRole('option', { name: 'Rahul Verma' });
    await userEvent.selectOptions(await screen.findByLabelText('Link Rahul Sharma to a person'), 'p1');
    await userEvent.click(screen.getByLabelText('Also match this name next time'));
    await userEvent.click(screen.getByRole('button', { name: 'Link Rahul Sharma to the chosen person' }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(bodyOf(posts(fetchMock)[0])).toEqual({ person_id: 'p1', remember_name: true });
  });

  it('adds them as a person with an email, prefilled from what was written', async () => {
    const fetchMock = mockApi({ '/staffing/identities/idn1/add-person': {}, '/staffing/identities': unknown, ...base });
    renderPage();
    await userEvent.click(await screen.findByRole('button', { name: 'Add as a person' }));
    const dialog = await screen.findByRole('dialog', { name: 'Add Rahul Sharma to People' });
    expect(within(dialog).getByLabelText('Name')).toHaveValue('Rahul Sharma');
    expect(within(dialog).getByLabelText('Employee ID (optional)')).toHaveValue('X-9');
    expect(within(dialog).getByText(/does not give them a login/)).toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText('Email address'), 'rahul.s@client.test');
    await userEvent.click(within(dialog).getByRole('button', { name: 'Add person' }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(bodyOf(posts(fetchMock)[0])).toEqual({ name: 'Rahul Sharma', email: 'rahul.s@client.test', employee_code: 'X-9' });
  });

  it('marks a name as outside staff or not a person', async () => {
    const fetchMock = mockApi({ '/staffing/identities/idn1/external': {}, '/staffing/identities/idn1/dismiss': {}, '/staffing/identities': unknown, ...base });
    renderPage();
    await userEvent.click(await screen.findByRole('button', { name: 'They work for a client' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Not a person' }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(2));
    expect(posts(fetchMock).map(([u]) => String(u).split('/').pop())).toEqual(['external', 'dismiss']);
  });

  it('undoes a recent decision and says what undo does not do', async () => {
    const fetchMock = mockApi({ '/staffing/identity-changes/idc1/undo': {}, '/staffing/identities': unknown, ...base });
    renderPage();
    const recent = await screen.findByRole('list', { name: 'Recent decisions' });
    expect(within(recent).getByText('Linked Rahul Sharma to Rahul Verma (1 sheet)')).toBeInTheDocument();
    expect(screen.getByText(/A reply that already went out is not taken back/)).toBeInTheDocument();
    expect(within(recent).getByText('Linked Dinesh Rao to Dinesh Rao (2 sheets); Dinesh Rao stays on People if you undo')).toBeInTheDocument();
    await userEvent.click(within(recent).getByRole('button', { name: 'Undo: Linked Rahul Sharma to Rahul Verma (1 sheet)' }));
    await waitFor(() => expect(posts(fetchMock)).toHaveLength(1));
    expect(String(posts(fetchMock)[0]?.[0])).toContain('/staffing/identity-changes/idc1/undo');
    expect(await screen.findByText(/^Undone. The sheets are back/)).toBeInTheDocument();
    await userEvent.click(within(recent).getByRole('button', { name: 'Undo: Marked Client Staff as outside staff' }));
    expect(await screen.findByText('Undone. Client Staff is back under "People we don\'t know yet".')).toBeInTheDocument();
  });

  it('shows nothing when the feature is off', async () => {
    mockApi({ '/staffing/identities': { enabled: false, items: [], recent: [] }, ...base });
    renderPage();
    await screen.findByText('Timesheets');
    expect(screen.queryByRole('heading', { name: "People we don't know yet" })).toBeNull();
  });

  it('is not asked for, and not shown, to anyone but the super admin', async () => {
    const fetchMock = mockApi({ '/staffing/identities': unknown, ...base, '/team/access': access('operator') });
    renderPage();
    await screen.findByText('Timesheets');
    await screen.findByLabelText('Month');
    expect(screen.queryByRole('heading', { name: "People we don't know yet" })).toBeNull();
    expect(fetchMock.mock.calls.some(([u]) => String(u).includes('/staffing/identities'))).toBe(false);
  });
});
