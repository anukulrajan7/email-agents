import { render, screen, waitFor, within } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { budgetPercent, usd } from './format';
import type { AiUsage } from './types';
import { UsagePage } from './UsagePage';

const day = (n: number, usdValue: number, calls: number) => ({ date: `2026-10-${String(n).padStart(2, '0')}`, usd: usdValue, calls });

const usage = (over: Partial<AiUsage> = {}): AiUsage => ({
  days: 30,
  total_usd: 0.42,
  today_usd: 0.5,
  budget_usd_per_day: 2,
  by_day: [day(12, 0.1, 30), day(13, 0, 0), day(14, 0.32, 80)],
  by_task: [
    { task: 'Drafts', usd: 0.3, calls: 40 },
    { task: 'Labelling', usd: 0.12, calls: 70 },
  ],
  per_email_usd: 0.0023,
  ...over,
});

const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));

function setup(opts: { role?: 'admin' | 'operator'; usage?: AiUsage } = {}) {
  const fetchMock = vi.fn((url: string) => {
    if (url.endsWith('/me')) {
      return json({
        user: { id: 'u', email: 'a@b.test', name: null },
        workspaces: [{ id: 'w1', name: 'W', role: opts.role ?? 'admin', created_at: '' }],
        auth_mode: 'dev',
      });
    }
    if (url.includes('/ai-usage')) return json(opts.usage ?? usage());
    return json({ items: [] });
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/usage']}>
        <Routes>
          <Route path="/w/:ws/usage" element={<UsagePage />} />
          <Route path="/w/:ws/inbox" element={<p>Inbox page</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('UsagePage', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('shows today against the budget, the 30-day total and the cost per email', async () => {
    const fetchMock = setup();

    expect(await screen.findByText('$0.50')).toBeInTheDocument();
    expect(screen.getByText('of $2.00')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: /budget/i })).toHaveAttribute('aria-valuenow', '25');
    expect(screen.getByText('$0.42')).toBeInTheDocument();
    expect(screen.getByText('110 AI calls')).toBeInTheDocument();
    expect(screen.getByText('$0.0023')).toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([url]) => url === '/api/v1/workspaces/w1/ai-usage?days=30')).toBe(true);
  });

  it('draws one bar per day and lists work by task with its share', async () => {
    setup();

    const chart = await screen.findByRole('img', { name: 'Cost per day for the last 3 days' });
    expect(chart.querySelectorAll('rect')).toHaveLength(3);
    expect(within(chart).getByText('14 Oct: $0.32, 80 calls')).toBeInTheDocument();

    const drafts = screen.getByRole('row', { name: /Drafts/ });
    expect(within(drafts).getByText('$0.30')).toBeInTheDocument();
    expect(within(drafts).getByText('71%')).toBeInTheDocument();
    expect(screen.getByRole('row', { name: /Labelling/ })).toBeInTheDocument();
  });

  it('warns when the daily budget is spent and never shows model names', async () => {
    setup({ usage: usage({ today_usd: 2.4 }) });

    expect(await screen.findByText(/Daily budget reached/)).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: /budget/i })).toHaveAttribute('aria-valuenow', '100');
    expect(document.body.textContent).not.toMatch(/qwen|kimi|haiku|deepseek|bge/i);
  });

  it('shows an empty state before any AI work', async () => {
    setup({ usage: usage({ total_usd: 0, today_usd: 0, by_task: [], by_day: [day(14, 0, 0)], per_email_usd: 0 }) });

    expect(await screen.findByText('No AI work yet')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: /budget/i })).toHaveAttribute('aria-valuenow', '0');
  });

  it('sends operators back to the inbox without asking for usage', async () => {
    const fetchMock = setup({ role: 'operator' });

    expect(await screen.findByText('Inbox page')).toBeInTheDocument();
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes('/ai-usage'))).toBe(false);
  });
});

describe('usage formatting', () => {
  it('formats small costs with enough precision and clamps the budget bar', () => {
    expect(usd(0)).toBe('$0.00');
    expect(usd(0.0023)).toBe('$0.0023');
    expect(usd(1.239)).toBe('$1.24');
    expect(budgetPercent(1, 2)).toBe(50);
    expect(budgetPercent(5, 2)).toBe(100);
    expect(budgetPercent(0, 0)).toBe(0);
    expect(budgetPercent(0.1, 0)).toBe(100);
  });
});
