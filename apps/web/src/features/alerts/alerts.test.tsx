import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AlertSettingsPanel } from './AlertSettingsPanel';
import { AlertsBadge } from './AlertsBadge';
import { AlertsPage } from './AlertsPage';
import type { Alert, AlertSettings } from './types';
import { hoursUntilTomorrowMorning, isEmail, validateSettings } from './validation';

const alert = (over: Partial<Alert>): Alert => ({
  id: 'a1',
  kind: 'important_unanswered',
  severity: 'high',
  title: 'Invoice question unanswered',
  body: 'No reply after 4 hours.',
  message_id: 'm1',
  message_subject: 'Invoice 42',
  status: 'open',
  escalation_level: 0,
  notified: ['ops@acme.com'],
  created_at: new Date(Date.now() - 3 * 3_600_000).toISOString(),
  resolved_at: null,
  ...over,
});

const settings = (over: Partial<AlertSettings> = {}): AlertSettings => ({
  enabled: true,
  recipients: ['ops@acme.com'],
  escalate_to: [],
  unanswered_hours: 4,
  escalate_after_hours: 24,
  daily_digest: true,
  digest_hour: 9,
  ...over,
});

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });

function mockFetch(handler: (url: string, init?: RequestInit) => unknown) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => Promise.resolve(json(handler(url, init))));
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

function renderWith(ui: React.ReactNode) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/alerts']}>
        <Routes>
          <Route path="/w/:ws/alerts" element={ui} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const bodiesOf = (fetchMock: ReturnType<typeof mockFetch>, method: string) =>
  fetchMock.mock.calls.filter(([, init]) => init?.method === method).map(([url, init]) => ({ url, body: init?.body ? JSON.parse(init.body as string) : undefined }));

afterEach(() => vi.unstubAllGlobals());

describe('AlertsBadge', () => {
  it('shows the open count from GET /alerts?status=open', async () => {
    const fetchMock = mockFetch(() => ({ items: [alert({})], open_count: 7 }));
    renderWith(<AlertsBadge ws="w1" />);

    expect(await screen.findByText('7 open alerts')).toBeInTheDocument();
    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/v1/workspaces/w1/alerts?status=open');
  });

  it('renders nothing when there are no open alerts', async () => {
    const fetchMock = mockFetch(() => ({ items: [], open_count: 0 }));
    const { container } = renderWith(<AlertsBadge ws="w1" />);

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    await waitFor(() => expect(container).toBeEmptyDOMElement());
  });

  it('falls back to the item count when open_count is missing', async () => {
    mockFetch(() => ({ items: [alert({ id: 'a' }), alert({ id: 'b' })] }));
    renderWith(<AlertsBadge ws="w1" />);
    expect(await screen.findByText('2 open alerts')).toBeInTheDocument();
  });
});

describe('AlertsPage', () => {
  const open = alert({ escalation_level: 1, notified: ['ops@acme.com', 'boss@acme.com'] });
  const list = (url: string) => (url.includes('status=resolved') ? { items: [alert({ id: 'r1', status: 'resolved', title: 'Old one', resolved_at: new Date().toISOString() })], open_count: 1 } : { items: [open], open_count: 1 });

  it('lists an alert with severity, escalation, recipients and a link to the email', async () => {
    mockFetch(list);
    renderWith(<AlertsPage />);

    const card = await screen.findByRole('group', { name: 'Invoice question unanswered' });
    expect(within(card).getByText('High')).toBeInTheDocument();
    expect(within(card).getByText('Escalated to manager')).toBeInTheDocument();
    expect(within(card).getByText('Notified: ops@acme.com, boss@acme.com')).toBeInTheDocument();
    expect(within(card).getByRole('link', { name: 'Invoice 42' })).toHaveAttribute('href', '/w/w1/inbox/m1');
  });

  it('resolve PATCHes status resolved', async () => {
    const fetchMock = mockFetch((url, init) => (init?.method === 'PATCH' ? alert({ status: 'resolved' }) : list(url)));
    renderWith(<AlertsPage />);

    await userEvent.click(await screen.findByRole('button', { name: 'Resolve' }));
    await waitFor(() => {
      const [call] = bodiesOf(fetchMock, 'PATCH');
      expect(call?.url).toContain('/workspaces/w1/alerts/a1');
      expect(call?.body).toEqual({ status: 'resolved' });
    });
  });

  it('snooze sends snooze_hours for 1h and 4h', async () => {
    const fetchMock = mockFetch((url, init) => (init?.method === 'PATCH' ? open : list(url)));
    renderWith(<AlertsPage />);

    const select = await screen.findByLabelText('Snooze: Invoice question unanswered');
    await userEvent.selectOptions(select, '1 hour');
    await waitFor(() => expect(bodiesOf(fetchMock, 'PATCH')).toHaveLength(1));
    await userEvent.selectOptions(select, '4 hours');
    await waitFor(() => expect(bodiesOf(fetchMock, 'PATCH')).toHaveLength(2));
    expect(bodiesOf(fetchMock, 'PATCH').map((c) => c.body)).toEqual([
      { status: 'open', snooze_hours: 1 },
      { status: 'open', snooze_hours: 4 },
    ]);
  });

  it('reopen a resolved alert PATCHes status open', async () => {
    const fetchMock = mockFetch((url, init) => (init?.method === 'PATCH' ? open : list(url)));
    renderWith(<AlertsPage />);

    await userEvent.click(await screen.findByRole('tab', { name: /Resolved/ }));
    await userEvent.click(await screen.findByRole('button', { name: 'Reopen' }));
    await waitFor(() => {
      const [call] = bodiesOf(fetchMock, 'PATCH');
      expect(call?.url).toContain('/alerts/r1');
      expect(call?.body).toEqual({ status: 'open' });
    });
  });

  it('shows an empty state when all clear', async () => {
    mockFetch(() => ({ items: [], open_count: 0 }));
    renderWith(<AlertsPage />);
    expect(await screen.findByText('All clear')).toBeInTheDocument();
  });
});

describe('AlertSettingsPanel', () => {
  it('rejects invalid email addresses and keeps valid ones as chips', async () => {
    mockFetch(() => settings());
    renderWith(<AlertSettingsPanel ws="w1" />);

    const escalate = await screen.findByLabelText('Escalate to');
    await userEvent.type(escalate, 'not-an-email{Enter}');
    expect(await screen.findByRole('alert')).toHaveTextContent('not-an-email is not a valid email address');
    expect(screen.queryByRole('button', { name: 'Remove not-an-email' })).not.toBeInTheDocument();

    await userEvent.clear(escalate);
    await userEvent.type(escalate, 'Boss@Acme.com{Enter}');
    expect(screen.getByRole('button', { name: 'Remove boss@acme.com' })).toBeInTheDocument();
    expect(screen.queryByText(/not a valid email/)).not.toBeInTheDocument();
  });

  it('requires a recipient and valid hours before saving', async () => {
    const fetchMock = mockFetch(() => settings({ recipients: [] }));
    renderWith(<AlertSettingsPanel ws="w1" />);

    await userEvent.click(await screen.findByRole('button', { name: 'Save settings' }));
    expect(await screen.findByText('Add at least one person to notify.')).toBeInTheDocument();

    const hours = screen.getByLabelText('Hours until alert for an unanswered important email');
    await userEvent.clear(hours);
    await userEvent.click(screen.getByRole('button', { name: 'Save settings' }));
    expect(await screen.findAllByText('Enter a whole number of hours, 1 to 720.')).toHaveLength(1);
    expect(bodiesOf(fetchMock, 'PUT')).toHaveLength(0);
  });

  it('saves with PUT', async () => {
    const fetchMock = mockFetch((_url, init) => (init?.method === 'PUT' ? JSON.parse(init.body as string) : settings()));
    renderWith(<AlertSettingsPanel ws="w1" />);

    const hours = await screen.findByLabelText('Hours until alert for an unanswered important email');
    await userEvent.clear(hours);
    await userEvent.type(hours, '2');
    await userEvent.click(screen.getByRole('button', { name: 'Save settings' }));

    await waitFor(() => {
      const [call] = bodiesOf(fetchMock, 'PUT');
      expect(call?.url).toContain('/workspaces/w1/alert-settings');
      expect(call?.body).toEqual(settings({ unanswered_hours: 2 }));
    });
  });

  it('sends a test alert and shows who received it', async () => {
    const fetchMock = mockFetch((url, init) =>
      init?.method === 'POST' && url.endsWith('/alert-settings/test') ? { sent_to: ['ops@acme.com'] } : settings(),
    );
    renderWith(<AlertSettingsPanel ws="w1" />);

    await userEvent.click(await screen.findByRole('button', { name: 'Send test alert' }));
    expect(await screen.findByText(/Sent to ops@acme.com/)).toBeInTheDocument();
    expect(bodiesOf(fetchMock, 'POST')).toHaveLength(1);
  });
});

describe('validation helpers', () => {
  it('validates emails', () => {
    expect(isEmail('a@b.co')).toBe(true);
    expect(isEmail('a@b')).toBe(false);
    expect(isEmail('a b@c.com')).toBe(false);
  });

  it('flags missing recipients only when enabled', () => {
    expect(validateSettings(settings({ recipients: [] })).recipients).toBeDefined();
    expect(validateSettings(settings({ recipients: [], enabled: false })).recipients).toBeUndefined();
    expect(validateSettings(settings({ escalate_after_hours: 0 })).escalate_after_hours).toBeDefined();
  });

  it('computes hours until 9am tomorrow', () => {
    expect(hoursUntilTomorrowMorning(new Date(2026, 9, 14, 17, 0, 0))).toBe(16);
    expect(hoursUntilTomorrowMorning(new Date(2026, 9, 14, 8, 30, 0))).toBe(25);
  });
});
