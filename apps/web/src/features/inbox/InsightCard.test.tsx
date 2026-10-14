import { render as rtlRender, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactElement } from 'react';
import { afterEach, vi } from 'vitest';
import type { Insight } from '../../api/types';
import { InsightCard } from './InsightCard';

const wrap = (ui: ReactElement) => <QueryClientProvider client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}>{ui}</QueryClientProvider>;
const render = (ui: ReactElement) => {
  const r = rtlRender(wrap(ui));
  return { ...r, rerender: (next: ReactElement) => r.rerender(wrap(next)) };
};
afterEach(() => vi.restoreAllMocks());

const insight: Insight = {
  summary: 'Vendor wants the September invoice approved.',
  asks: ['Approve invoice #123', 'Confirm payment date'],
  deadline: 'Friday 5pm',
  needs_reply: true,
  missing_info: ['PO number'],
};

describe('InsightCard', () => {
  it('shows summary, asks, deadline, reply chip and missing info', () => {
    render(<InsightCard ws="w1" messageId="m1" insight={insight} status="processed" />);
    expect(screen.getByText("What they're asking")).toBeInTheDocument();
    expect(screen.getByText('Vendor wants the September invoice approved.')).toBeInTheDocument();
    expect(screen.getAllByRole('listitem').map((li) => li.textContent)).toEqual(['Approve invoice #123', 'Confirm payment date', 'PO number']);
    expect(screen.getByText('Deadline: Friday 5pm')).toBeInTheDocument();
    expect(screen.getByText('Needs reply')).toBeInTheDocument();
    expect(screen.getByText('PO number')).toBeInTheDocument();
  });

  it('shows a chat-style summary with rupee amounts as written', () => {
    const summary = 'You spent ₹8,138.76 on your card ending 4755 at Amazon on 12 Oct.';
    render(<InsightCard ws="w1" messageId="m1" insight={{ ...insight, summary, asks: [], missing_info: [], deadline: null }} status="processed" />);
    expect(screen.getByText(summary)).toBeInTheDocument();
  });

  it('shows Replied instead of Needs reply once we answered', () => {
    render(<InsightCard ws="w1" messageId="m1" insight={insight} status="processed" replied />);
    expect(screen.getByText('Replied')).toBeInTheDocument();
    expect(screen.queryByText('Needs reply')).toBeNull();
  });

  it('says when no reply is needed and omits empty sections', () => {
    render(<InsightCard ws="w1" messageId="m1" insight={{ ...insight, needs_reply: false, deadline: null, asks: [], missing_info: [] }} status="processed" />);
    expect(screen.getByText('No reply needed')).toBeInTheDocument();
    expect(screen.queryByText('Needs reply')).toBeNull();
    expect(screen.queryByText(/Deadline/)).toBeNull();
    expect(screen.queryByRole('list')).toBeNull();
  });

  it('shows a skeleton and reads the email right away when the insight is missing', async () => {
    const fetchMock = vi.spyOn(globalThis, 'fetch').mockResolvedValue(new Response(JSON.stringify({ insight }), { status: 200, headers: { 'content-type': 'application/json' } }));
    render(<InsightCard ws="w1" messageId="m1" insight={null} status="classified" />);
    expect(screen.getByText('Reading this email…')).toBeInTheDocument();
    await waitFor(() => expect(fetchMock).toHaveBeenCalledTimes(1));
    const [url, init] = fetchMock.mock.calls[0] ?? [];
    expect(String(url)).toContain('/workspaces/w1/messages/m1/insight');
    expect(init?.method).toBe('POST');
  });

  it('renders nothing when the API sends no insight field or processing failed', () => {
    const { container, rerender } = render(<InsightCard ws="w1" messageId="m1" insight={undefined} status="processed" />);
    expect(container).toBeEmptyDOMElement();
    rerender(<InsightCard ws="w1" messageId="m1" insight={null} status="failed" />);
    expect(container).toBeEmptyDOMElement();
  });
});
