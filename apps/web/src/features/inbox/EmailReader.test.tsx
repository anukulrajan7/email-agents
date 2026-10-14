import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MessageDetail } from '../../api/types';
import { EmailReader } from './EmailReader';

const message = (over: Partial<MessageDetail> = {}): MessageDetail => ({
  id: 'm1',
  source: { id: 's1', kind: 'nango_gmail', can_create_provider_draft: true, can_send: true },
  from: 'Google <no-reply@accounts.google.com>',
  original_from: null,
  to: ['me@wokay.com', 'Team <team@wokay.com>', 'a@x.com'],
  cc: ['boss@wokay.com'],
  subject: 'Security alert',
  body_text: 'New sign-in',
  body_html_safe: null,
  received_at: '2026-10-13T16:55:00Z',
  processing_status: 'processed',
  attachments: [{ id: 'a1', name: 'report.pdf', media_type: 'application/pdf', size_bytes: 2048, scan_status: 'clean' }],
  classification: {
    category: 'other',
    priority: 'normal',
    requires_reply: false,
    confidence: 0.9,
    needs_review: false,
    source: 'llm',
    model_id: 'moonshotai/kimi-k2',
    updated_at: 't',
  },
  draft: null,
  review: null,
  workflow_run: null,
  audit: [],
  contact: null,
  contact_memories: [],
  insight: null,
  ...over,
});

function setup(m: MessageDetail) {
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(JSON.stringify({ items: [] })))));
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter>
        <EmailReader ws="w1" message={m} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe('EmailReader', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('renders a Gmail-style header with avatar initial, name, address and recipients', () => {
    setup(message());
    expect(screen.getByRole('heading', { level: 2, name: 'Security alert' })).toBeInTheDocument();
    expect(screen.getByText('Google')).toHaveClass('font-bold');
    expect(screen.getByText('<no-reply@accounts.google.com>')).toHaveClass('text-ink-muted');
    expect(screen.getByText('G')).toBeInTheDocument();
    expect(screen.getByText('to me, Team +1')).toBeInTheDocument();
  });

  it('expands a details popover with from, to, cc and date', async () => {
    setup(message());
    await userEvent.click(screen.getByRole('button', { name: 'Show details' }));
    const details = screen.getByRole('group', { name: 'Message details' });
    expect(within(details).getByText('from:')).toBeInTheDocument();
    expect(within(details).getByText('cc:')).toBeInTheDocument();
    expect(within(details).getByText('boss@wokay.com')).toBeInTheDocument();
    expect(within(details).getByText('date:')).toBeInTheDocument();
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('group', { name: 'Message details' })).toBeNull();
  });

  it('shows the insight skeleton, attachments and no pipeline tracker or model names', () => {
    setup(message({ processing_status: 'processed' }));
    expect(screen.getByText('Reading this email…')).toBeInTheDocument();
    expect(screen.getByText('report.pdf')).toBeInTheDocument();
    expect(screen.queryByText(/Workflow done/)).toBeNull();
    expect(screen.queryByRole('list', { name: 'Processing route' })).toBeNull();
    expect(document.body.textContent).not.toMatch(/kimi|moonshot|qwen|openrouter/i);
  });

  it('uses the same body container for HTML and text emails', () => {
    const { unmount } = setup(message({ body_html_safe: '<p>Hello</p>' }));
    const htmlRegion = screen.getByRole('region', { name: 'Email body' });
    expect(within(htmlRegion).getByTitle('Email content')).toBeInTheDocument();
    unmount();
    setup(message({ body_html_safe: null }));
    const textRegion = screen.getByRole('region', { name: 'Email body' });
    expect(within(textRegion).getByText('New sign-in')).toBeInTheDocument();
    expect(textRegion.className).toBe(htmlRegion.className);
  });
});

describe('timesheet chip', () => {
  it('shows what was recorded from the email and links to the record', () => {
    setup(message({ timesheet_records: [{ id: 'tsr1', status: 'recorded', period_start: '2026-09-01', period_end: '2026-09-30', total_hours: 168 }] }));
    const chip = screen.getByRole('link', { name: 'Timesheet recorded · Sep 2026 · 168 h' });
    expect(chip).toHaveAttribute('href', '/w/w1/timesheets?record=tsr1');
  });

  it('says when a timesheet needs a check, and shows nothing for a replaced one', () => {
    setup(
      message({
        timesheet_records: [
          { id: 'a', status: 'replaced', period_start: '2026-09-01', period_end: '2026-09-30', total_hours: 100 },
          { id: 'b', status: 'needs_check', period_start: '2026-09-01', period_end: '2026-09-12', total_hours: null },
        ],
      }),
    );
    expect(screen.getByRole('link', { name: 'Timesheet needs a check · 1 Sep – 12 Sep 2026' })).toBeInTheDocument();
    expect(screen.getAllByRole('link', { name: /Timesheet/ })).toHaveLength(1);
  });
});
