import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Draft, MessageDetail, SafetyReport } from '../../api/types';
import { DraftPanel } from './DraftPanel';

const draft: Draft = {
  id: 'd1',
  message_id: 'm1',
  status: 'suggested',
  body_text: 'Hello there\nline two',
  model_id: null,
  memories_used: [],
  created_at: 't0',
  updated_at: 't1',
};

const message = (over: Partial<MessageDetail> = {}): MessageDetail => ({
  id: 'm1',
  source: { id: 's1', kind: 'nango_gmail', can_create_provider_draft: true, can_send: true },
  from: 'alice@example.com',
  original_from: null,
  to: [],
  subject: 'Re: Lunch',
  body_text: 'hi',
  body_html_safe: null,
  received_at: 't0',
  processing_status: 'workflow_completed',
  attachments: [],
  classification: null,
  draft,
  review: null,
  workflow_run: null,
  audit: [],
  contact: null,
  contact_memories: [],
  ...over,
});

const cleanReport: SafetyReport = { blocking: [], warnings: [] };

function setup(m: MessageDetail, report: SafetyReport = cleanReport) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    const body = url.endsWith('/check')
      ? report
      : url.endsWith('/undo-send')
        ? { status: 'cancelled', draft_id: 'd1' }
        : url.endsWith('/send')
          ? { status: 'scheduled', draft_id: 'd1', send_at: new Date(Date.now() + 30_000).toISOString() }
          : init?.method === 'POST'
            ? { status: 'queued' }
            : {};
    return Promise.resolve(new Response(JSON.stringify(body)));
  });
  vi.stubGlobal('fetch', fetchMock);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <DraftPanel ws="w1" message={m} onAwaitingChange={() => undefined} />
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('DraftPanel', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('sends at once with no dialog and no check step, posting {confirm:true}', async () => {
    const fetchMock = setup(message());
    await userEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/drafts/d1/send',
        expect.objectContaining({ method: 'POST', body: JSON.stringify({ confirm: true }) }),
      ),
    );
  });

  it('checks the draft when it loads and shows a quiet heads-up that never blocks Send', async () => {
    const fetchMock = setup(message(), { blocking: [], warnings: [{ code: 'unsupported_promise', message: 'This mentions a payment date - double-check before sending.', fix: null }] });
    expect(await screen.findByText('This mentions a payment date - double-check before sending.')).toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledWith('/api/v1/workspaces/w1/drafts/d1/check', expect.objectContaining({ method: 'POST' }));
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
  });

  it('shows nothing when the check is clean', async () => {
    const fetchMock = setup(message());
    await waitFor(() => expect(fetchMock).toHaveBeenCalledWith('/api/v1/workspaces/w1/drafts/d1/check', expect.anything()));
    expect(screen.queryByRole('list', { name: 'Heads-up' })).toBeNull();
  });

  describe('send and undo', () => {
    it('shows "Sending in 30 s · Undo" after sending, and Undo cancels it', async () => {
      const fetchMock = setup(message());
      await userEvent.click(screen.getByRole('button', { name: 'Send' }));
      expect(await screen.findByText(/Sending in (30|29) s/)).toBeInTheDocument();
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(screen.getByLabelText('Draft reply')).toHaveAttribute('readonly');
      await userEvent.click(screen.getByRole('button', { name: 'Undo' }));
      await waitFor(() =>
        expect(fetchMock).toHaveBeenCalledWith('/api/v1/workspaces/w1/drafts/d1/undo-send', expect.objectContaining({ method: 'POST' })),
      );
      await waitFor(() => expect(screen.queryByText(/Sending in/)).toBeNull());
      expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    });

    it('counts down a scheduled draft and switches to "Sending…" at zero', () => {
      vi.useFakeTimers();
      try {
        setup(message({ draft: { ...draft, status: 'scheduled', send_at: new Date(Date.now() + 3_000).toISOString() } }));
        expect(screen.getByText('Sending in 3 s')).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: 'Send' })).toBeNull();
        act(() => vi.advanceTimersByTime(1_000));
        expect(screen.getByText('Sending in 2 s')).toBeInTheDocument();
        act(() => vi.advanceTimersByTime(2_000));
        expect(screen.getByText('Sending…')).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: 'Undo' })).toBeNull();
      } finally {
        vi.useRealTimers();
      }
    });

    it('shows why a scheduled send did not go out', () => {
      setup(message({ draft: { ...draft, status: 'edited', send_error: 'Not sent: someone already replied.' } }));
      expect(screen.getByText('Not sent: someone already replied.')).toBeInTheDocument();
    });
  });

  it('saves to Gmail drafts via the provider-draft endpoint', async () => {
    const fetchMock = setup(message());
    await userEvent.click(screen.getByRole('button', { name: 'More' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Save to Gmail drafts' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/drafts/d1/provider-draft',
        expect.objectContaining({ method: 'POST' }),
      ),
    );
    expect(await screen.findByText('Saved in your Gmail Drafts — you can finish it there too.')).toBeInTheDocument();
  });

  it('Write a better draft asks for the premium writer', async () => {
    const fetchMock = setup(message());
    await userEvent.click(screen.getByRole('button', { name: 'Write a better draft' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/messages/m1/draft',
        expect.objectContaining({ method: 'POST', body: JSON.stringify({ quality: 'premium' }) }),
      ),
    );
  });

  it('Regenerate sends no body', async () => {
    const fetchMock = setup(message());
    await userEvent.click(screen.getByRole('button', { name: 'Regenerate' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/messages/m1/draft',
        expect.objectContaining({ method: 'POST', body: undefined }),
      ),
    );
  });

  it('offers no better-draft button before a draft exists', () => {
    setup(message({ draft: null }));
    expect(screen.queryByRole('button', { name: 'Write a better draft' })).toBeNull();
    expect(screen.getByRole('button', { name: 'Generate draft' })).toBeInTheDocument();
  });

  it('renders sent drafts read-only', () => {
    setup(message({ draft: { ...draft, status: 'sent' } }));
    expect(screen.getByLabelText('Draft reply')).toHaveAttribute('readonly');
    expect(screen.queryByRole('button', { name: 'Send' })).toBeNull();
    expect(screen.getByText(/Sent from Gmail on/)).toBeInTheDocument();
  });

  describe('[NEEDS: …] placeholders', () => {
    const needsDraft: Draft = {
      ...draft,
      body_text: 'Hi Sam,\nThe rate is [NEEDS: hourly rate for Q4].\nStart date: [NEEDS: start date]',
      unanswered: ['hourly rate for Q4', 'start date'],
    };

    it('highlights placeholders, lists the checklist and disables Send but not Gmail drafts', async () => {
      setup(message({ draft: needsDraft }));
      const marks = within(screen.getByTestId('draft-highlights')).getAllByText(/\[NEEDS:/);
      expect(marks).toHaveLength(2);
      expect(marks[0]?.tagName).toBe('MARK');
      const checklist = screen.getByRole('region', { name: 'Fill these before sending' });
      expect(within(checklist).getAllByRole('listitem')).toHaveLength(2);
      const send = screen.getByRole('button', { name: 'Send' });
      expect(send).toBeDisabled();
      expect(send).toHaveAccessibleDescription(/Fill every dashed/);
      await userEvent.click(screen.getByRole('button', { name: 'More' }));
      expect(screen.getByRole('menuitem', { name: 'Save to Gmail drafts' })).toBeEnabled();
    });

    it('enables Send once every placeholder is filled and ticks the checklist', async () => {
      setup(message({ draft: { ...needsDraft, body_text: 'Rate is [NEEDS: hourly rate for Q4]', unanswered: ['hourly rate for Q4'] } }));
      await userEvent.click(screen.getByRole('button', { name: 'Edit' }));
      const field = screen.getByLabelText('Draft reply');
      await userEvent.clear(field);
      await userEvent.type(field, 'Rate is 45 USD');
      expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
      expect(screen.getByText('(done)')).toBeInTheDocument();
      expect(screen.queryByText(/Fill every dashed/)).toBeNull();
    });

    it('derives the checklist from the body when the API sends no unanswered list', () => {
      setup(message({ draft: { ...needsDraft, unanswered: undefined } }));
      expect(within(screen.getByRole('region', { name: 'Fill these before sending' })).getAllByRole('listitem')).toHaveLength(2);
    });

    it('shows a short note, not blanks, when a draft could not confirm something', () => {
      setup(message({ draft: { ...draft, unanswered: ['invoice amount'] } }));
      expect(screen.getByRole('note')).toHaveTextContent("Couldn't confirm: invoice amount");
      expect(screen.queryByRole('region', { name: 'Fill these before sending' })).toBeNull();
      expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    });

    it('does not show the checklist for a clean draft', () => {
      setup(message());
      expect(screen.queryByRole('region', { name: 'Fill these before sending' })).toBeNull();
      expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    });
  });
});
