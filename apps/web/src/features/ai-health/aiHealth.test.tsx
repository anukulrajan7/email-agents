import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { percent, seconds } from './format';
import type { AiHealth } from './types';
import { AiHealthPage } from './AiHealthPage';

const health = (over: Partial<AiHealth> = {}): AiHealth => ({
  days: 14,
  by_kind: [
    { kind: 'leak_blocked', label: 'Possible leak blocked', severity: 'high', total: 2, open: 1 },
    { kind: 'refusal', label: 'AI declined a request', severity: 'low', total: 0, open: 0 },
  ],
  by_day: [
    { date: '2026-10-13', counts: { leak_blocked: 1 }, total: 1 },
    { date: '2026-10-14', counts: { leak_blocked: 1 }, total: 1 },
  ],
  open_total: 1,
  open: [
    {
      id: 'inc_1',
      kind: 'leak_blocked',
      label: 'Possible leak blocked',
      severity: 'high',
      agent_id: 'agt_1',
      agent_name: 'Support lead',
      case_id: null,
      message_id: null,
      details: { items: 2 },
      created_at: Date.now() - 60_000,
    },
  ],
  cost: { today_usd: 1.2, per_email_today_usd: 0.0007, per_email_7d_usd: 0.0005, avg_daily_7d_usd: 0.4, spike: true },
  tasks: [{ task: 'draft', label: 'Drafts', calls: 40, p50_ms: 1200, p95_ms: 5400, error_rate: 0.05, refusal_rate: 0, schema_failure_rate: 0.0025 }],
  shadow: {
    compared: 80,
    failed: 2,
    label_agreement: 0.9,
    needs_reply_agreement: 0.95,
    priority_agreement: 0.85,
    min_agreement: 0.95,
    min_samples: 200,
    ready_to_switch: false,
  },
  budget: { daily_usd: 2, used_usd: 1.2, used_pct: 0.6 },
  ...over,
});

const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));

function setup(opts: { role?: 'admin' | 'operator'; data?: AiHealth } = {}) {
  const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
    if (url.endsWith('/me')) {
      return json({
        user: { id: 'u', email: 'a@b.test', name: null },
        workspaces: [{ id: 'w1', name: 'W', role: opts.role ?? 'admin', created_at: '' }],
        auth_mode: 'dev',
      });
    }
    if (url.includes('/ai-health')) return json(opts.data ?? health());
    if (url.includes('/resolve')) return json({ id: 'inc_1', resolved: true });
    return json({ items: [] });
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/ai-health']}>
        <Routes>
          <Route path="/w/:ws/ai-health" element={<AiHealthPage />} />
          <Route path="/w/:ws/inbox" element={<p>Inbox page</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('AiHealthPage', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('shows open incidents, cost per email and the daily budget', async () => {
    setup();

    expect(await screen.findByText('Open incidents')).toBeInTheDocument();
    expect(screen.getByText('$0.0007')).toBeInTheDocument();
    expect(screen.getByText('More than twice the weekly average')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: /budget/i })).toHaveAttribute('aria-valuenow', '60');
    expect(screen.getByText('on Support lead')).toBeInTheDocument();
    expect(screen.getByText('items: 2')).toBeInTheDocument();
  });

  it('draws the incident chart with a text table as the accessible alternative', async () => {
    setup();

    const chart = await screen.findByRole('img', { name: 'Incidents per day for the last 2 days' });
    expect(chart.querySelectorAll('rect')).toHaveLength(2);
    const row = screen.getByRole('row', { name: /Possible leak blocked/ });
    expect(within(row).getAllByText('2').length).toBeGreaterThan(0);
  });

  it('lists speed and error rates per task and the shadow result', async () => {
    setup();

    const row = await screen.findByRole('row', { name: /Drafts/ });
    expect(within(row).getByText('1.2 s')).toBeInTheDocument();
    expect(within(row).getByText('5.4 s')).toBeInTheDocument();
    expect(within(row).getByText('5%')).toBeInTheDocument();
    expect(screen.getByText('Still comparing')).toBeInTheDocument();
  });

  it('marks an incident handled', async () => {
    const fetchMock = setup();

    await userEvent.click(await screen.findByRole('button', { name: /Mark handled/ }));

    await waitFor(() =>
      expect(fetchMock.mock.calls.some(([url, init]) => String(url).endsWith('/ai-incidents/inc_1/resolve') && init?.method === 'POST')).toBe(true),
    );
  });

  it('shows an all-clear when nothing is open, and names no models', async () => {
    setup({ data: health({ open_total: 0, open: [] }) });

    expect(await screen.findByText('Nothing needs a look')).toBeInTheDocument();
    expect(document.body.textContent?.toLowerCase()).not.toMatch(/gpt|claude|deepseek|qwen|gemini/);
  });

  it('sends people who are not admins back to the inbox', async () => {
    setup({ role: 'operator' });
    expect(await screen.findByText('Inbox page')).toBeInTheDocument();
  });

  it('formats rates and speeds plainly', () => {
    expect(percent(0)).toBe('0%');
    expect(percent(0.004)).toBe('<1%');
    expect(percent(0.1667)).toBe('17%');
    expect(seconds(null)).toBe('-');
    expect(seconds(420)).toBe('420 ms');
    expect(seconds(5400)).toBe('5.4 s');
  });
});
