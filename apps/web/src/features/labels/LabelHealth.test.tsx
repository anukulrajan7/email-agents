import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Label } from '../../api/types';
import { LabelsPage } from './LabelsPage';
import { reliability } from './LabelHealth';
import type { LabelHealthReport, LabelHealthRow } from './healthTypes';

const lab = (over: Partial<Label>): Label => ({
  id: 'l1',
  key: 'timesheet',
  name: 'Timesheet',
  description: 'Weekly timesheets from staff',
  color: 'slate',
  position: 1,
  system: false,
  archived: false,
  message_count: 2,
  ...over,
});

const labels = [
  lab({}),
  lab({ id: 'l2', key: 'leave', name: 'Leave', description: 'Leave and time off requests', color: 'sage', position: 2 }),
  lab({ id: 'l3', key: 'other', name: 'Other', description: 'Anything that fits nowhere else', color: 'rule', position: 3, system: true }),
];

const trend = (counts: number[]) => counts.map((count, i) => ({ date: `2026-10-0${i + 1}`, count }));

const row = (over: Partial<LabelHealthRow>): LabelHealthRow => ({
  key: 'timesheet',
  name: 'Timesheet',
  count: 52,
  share: 0.13,
  auto: 50,
  corrected_away: 3,
  corrected_into: 1,
  correction_rate: 0.06,
  avg_confidence: 0.91,
  trend: trend([2, 4, 3]),
  ...over,
});

const report = (over: Partial<LabelHealthReport> = {}): LabelHealthReport => ({
  days: 30,
  total: 412,
  other_share: 0.31,
  auto_share: 0.82,
  labels: [
    row({}),
    row({ key: 'leave', name: 'Leave', count: 20, share: 0.05, auto: 12, corrected_away: 8, corrected_into: 0, correction_rate: 0.4 }),
  ],
  other_trend: trend([5, 6, 7]),
  advice: [
    { kind: 'add_label', label_key: null, text: '31% of mail lands in Other — try Suggest labels.' },
    { kind: 'label_unreliable', label_key: 'leave', text: 'People correct Leave often — sharpen its description.' },
  ],
  ...over,
});

const json = (body: unknown, status = 200) => Promise.resolve(new Response(JSON.stringify(body), { status }));

function setup(opts: { role?: 'admin' | 'operator'; health?: LabelHealthReport } = {}) {
  const fetchMock = vi.fn((url: string) => {
    if (url.endsWith('/me')) {
      return json({
        user: { id: 'u', email: 'a@b.test', name: null },
        workspaces: [{ id: 'w1', name: 'W', role: opts.role ?? 'admin', created_at: '' }],
        auth_mode: 'password',
      });
    }
    if (url.includes('/labels/health')) return json(opts.health ?? report());
    return json({ items: labels });
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/labels']}>
        <Routes>
          <Route path="/w/:ws/labels" element={<LabelsPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { fetchMock, user: userEvent.setup() };
}

async function openHealth(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole('tab', { name: 'Health' }));
}

describe('reliability', () => {
  it('maps correction rate to plain words', () => {
    expect(reliability({ auto: 50, corrected_away: 3, correction_rate: 0.06 }).text).toBe('Reliable');
    expect(reliability({ auto: 40, corrected_away: 6, correction_rate: 0.13 }).text).toBe('Watch');
    expect(reliability({ auto: 6, corrected_away: 4, correction_rate: 0.4 }).text).toBe('Unreliable');
    expect(reliability({ auto: 2, corrected_away: 1, correction_rate: 0.33 }).text).toBe('Watch');
    expect(reliability({ auto: 0, corrected_away: 0, correction_rate: 0 }).text).toBe('No data');
  });
});

describe('Label health', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('shows the headline, a row per label with a plain-words badge, sparklines and advice', async () => {
    const { fetchMock, user } = setup();
    await openHealth(user);
    expect(await screen.findByText('82%')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('82% of mail is labelled automatically, 31% lands in Other.');

    const table = screen.getByRole('table', { name: 'Label health' });
    const timesheet = within(table).getByText('Timesheet').closest('tr') as HTMLElement;
    expect(within(timesheet).getByText('52')).toBeInTheDocument();
    expect(within(timesheet).getByText('6%')).toBeInTheDocument();
    expect(within(timesheet).getByText('Reliable')).toBeInTheDocument();
    expect(within(timesheet).getByRole('img', { name: 'Timesheet: 52 emails in the last 30 days' })).toBeInTheDocument();
    const leave = within(table).getByText('Leave').closest('tr') as HTMLElement;
    expect(within(leave).getByText('Unreliable')).toBeInTheDocument();
    expect(within(table).getByText('Other').closest('tr')).toHaveTextContent('18');
    expect(within(screen.getByRole('list', { name: 'Advice' })).getAllByRole('listitem')).toHaveLength(2);
    expect(fetchMock.mock.calls.some(([u]) => String(u).includes('/labels/health?days=30'))).toBe(true);
  });

  it('refetches for the selected range', async () => {
    const { fetchMock, user } = setup();
    await openHealth(user);
    await screen.findByRole('table', { name: 'Label health' });
    await user.click(screen.getByRole('button', { name: '7 days' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).includes('/labels/health?days=7'))).toBe(true));
    expect(screen.getByRole('button', { name: '7 days' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('opens Suggest labels from the add-label advice', async () => {
    const scroll = vi.fn();
    Element.prototype.scrollIntoView = scroll;
    const { user } = setup();
    await openHealth(user);
    await user.click(await screen.findByRole('button', { name: 'Suggest labels' }));
    const section = await screen.findByRole('region', { name: 'Suggested labels' });
    await waitFor(() => expect(scroll).toHaveBeenCalled());
    expect(section).toHaveFocus();
    expect(screen.getByRole('tab', { name: 'Labels' })).toHaveAttribute('aria-selected', 'true');
  });

  it('opens the label edit form from the unreliable-label advice', async () => {
    Element.prototype.scrollIntoView = vi.fn();
    const { user } = setup();
    await openHealth(user);
    await user.click(await screen.findByRole('button', { name: 'Edit label' }));
    expect(await screen.findByRole('button', { name: 'Save changes' })).toBeInTheDocument();
    expect(screen.getAllByLabelText('Name').map((el) => (el as HTMLInputElement).value)).toContain('Leave');
  });

  it('shows advice without actions to operators', async () => {
    const { user } = setup({ role: 'operator' });
    await openHealth(user);
    expect(await screen.findByText('People correct Leave often — sharpen its description.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Edit label' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Suggest labels' })).not.toBeInTheDocument();
  });

  it('explains an empty window', async () => {
    const { user } = setup({ health: report({ total: 0, labels: [], advice: [], other_share: 0, auto_share: 0 }) });
    await openHealth(user);
    expect(await screen.findByText('No mail to measure yet')).toBeInTheDocument();
  });
});
