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

  it('requires confirming in the dialog before sending, and posts {confirm:true}', async () => {
    const fetchMock = setup(message());
    await userEvent.click(screen.getByRole('button', { name: 'Send reply…' }));
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(screen.getByText('Re: Lunch')).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalledWith('/api/v1/workspaces/w1/drafts/d1/send', expect.anything());
    expect(await screen.findByText('Checks passed.')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Send' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/drafts/d1/send',
        expect.objectContaining({ method: 'POST', body: JSON.stringify({ confirm: true }) }),
      ),
    );
  });

  it('closes the dialog with Escape and returns focus to the trigger', async () => {
    const fetchMock = setup(message());
    const trigger = screen.getByRole('button', { name: 'Send reply…' });
    await userEvent.click(trigger);
    await userEvent.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(trigger).toHaveFocus();
    expect(fetchMock).not.toHaveBeenCalledWith('/api/v1/workspaces/w1/drafts/d1/send', expect.anything());
  });

  describe('send safety and undo', () => {
    it('checks the current text and shows blocking issues in red with fixes, disabling Send', async () => {
      const fetchMock = setup(message(), {
        blocking: [{ code: 'placeholder', message: 'The reply still has template placeholders: {name}.', fix: 'Replace each placeholder.' }],
        warnings: [{ code: 'tone', message: 'The tone may not be professional.', fix: null }],
      });
      await userEvent.click(screen.getByRole('button', { name: 'Send reply…' }));
      await waitFor(() =>
        expect(fetchMock).toHaveBeenCalledWith(
          '/api/v1/workspaces/w1/drafts/d1/check',
          expect.objectContaining({ method: 'POST', body: JSON.stringify({ body_text: draft.body_text }) }),
        ),
      );
      const blocking = await screen.findByRole('region', { name: 'Fix before sending' });
      expect(within(blocking).getByText(/template placeholders/)).toBeInTheDocument();
      expect(within(blocking).getByText('Replace each placeholder.')).toBeInTheDocument();
      expect(blocking.className).toContain('text-danger');
      expect(screen.getByRole('region', { name: 'Check before sending' }).className).toContain('bg-review-tint');
      expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    });

    it('allows sending with warnings only', async () => {
      setup(message(), {
        blocking: [],
        warnings: [{ code: 'attachment_mentioned', message: 'The reply mentions an attachment.', fix: 'Remove the mention.' }],
      });
      await userEvent.click(screen.getByRole('button', { name: 'Send reply…' }));
      const warnings = await screen.findByRole('region', { name: 'Check before sending' });
      expect(within(warnings).getByText('The reply mentions an attachment.')).toBeInTheDocument();
      expect(screen.queryByRole('region', { name: 'Fix before sending' })).toBeNull();
      expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    });

    it('shows "Sending in 30 s · Undo" after sending, and Undo cancels it', async () => {
      const fetchMock = setup(message());
      await userEvent.click(screen.getByRole('button', { name: 'Send reply…' }));
      await screen.findByText('Checks passed.');
      await userEvent.click(screen.getByRole('button', { name: 'Send' }));
      expect(await screen.findByText(/Sending in (30|29) s/)).toBeInTheDocument();
      expect(screen.queryByRole('dialog')).toBeNull();
      expect(screen.getByLabelText('Draft reply')).toHaveAttribute('readonly');
      await userEvent.click(screen.getByRole('button', { name: 'Undo' }));
      await waitFor(() =>
        expect(fetchMock).toHaveBeenCalledWith('/api/v1/workspaces/w1/drafts/d1/undo-send', expect.objectContaining({ method: 'POST' })),
      );
      await waitFor(() => expect(screen.queryByText(/Sending in/)).toBeNull());
      expect(screen.getByRole('button', { name: 'Send reply…' })).toBeEnabled();
    });

    it('counts down a scheduled draft and switches to "Sending…" at zero', () => {
      vi.useFakeTimers();
      try {
        setup(message({ draft: { ...draft, status: 'scheduled', send_at: new Date(Date.now() + 3_000).toISOString() } }));
        expect(screen.getByText('Sending in 3 s')).toBeInTheDocument();
        expect(screen.queryByRole('button', { name: 'Send reply…' })).toBeNull();
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
    await userEvent.click(screen.getByRole('button', { name: 'Save to Gmail drafts' }));
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
    expect(screen.queryByRole('button', { name: 'Send reply…' })).toBeNull();
    expect(screen.getByText(/Sent from Gmail on/)).toBeInTheDocument();
  });

  describe('[NEEDS: …] placeholders', () => {
    const needsDraft: Draft = {
      ...draft,
      body_text: 'Hi Sam,\nThe rate is [NEEDS: hourly rate for Q4].\nStart date: [NEEDS: start date]',
      unanswered: ['hourly rate for Q4', 'start date'],
    };

    it('highlights placeholders, lists the checklist and disables Send but not Gmail drafts', () => {
      setup(message({ draft: needsDraft }));
      const marks = within(screen.getByTestId('draft-highlights')).getAllByText(/\[NEEDS:/);
      expect(marks).toHaveLength(2);
      expect(marks[0]?.tagName).toBe('MARK');
      const checklist = screen.getByRole('region', { name: 'Fill these before sending' });
      expect(within(checklist).getAllByRole('listitem')).toHaveLength(2);
      const send = screen.getByRole('button', { name: 'Send reply…' });
      expect(send).toBeDisabled();
      expect(send).toHaveAccessibleDescription(/Fill every dashed/);
      expect(screen.getByRole('button', { name: 'Save to Gmail drafts' })).toBeEnabled();
    });

    it('enables Send once every placeholder is filled and ticks the checklist', async () => {
      setup(message({ draft: { ...needsDraft, body_text: 'Rate is [NEEDS: hourly rate for Q4]', unanswered: ['hourly rate for Q4'] } }));
      const field = screen.getByLabelText('Draft reply');
      await userEvent.clear(field);
      await userEvent.type(field, 'Rate is 45 USD');
      expect(screen.getByRole('button', { name: 'Send reply…' })).toBeEnabled();
      expect(screen.getByText('(done)')).toBeInTheDocument();
      expect(screen.queryByText(/Fill every dashed/)).toBeNull();
    });

    it('derives the checklist from the body when the API sends no unanswered list', () => {
      setup(message({ draft: { ...needsDraft, unanswered: undefined } }));
      expect(within(screen.getByRole('region', { name: 'Fill these before sending' })).getAllByRole('listitem')).toHaveLength(2);
    });

    it('does not show the checklist for a clean draft', () => {
      setup(message());
      expect(screen.queryByRole('region', { name: 'Fill these before sending' })).toBeNull();
      expect(screen.getByRole('button', { name: 'Send reply…' })).toBeEnabled();
    });
  });
});
