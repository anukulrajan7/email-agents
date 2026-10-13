import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Draft, MessageDetail } from '../../api/types';
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

function setup(m: MessageDetail) {
  const fetchMock = vi.fn((_url: string, init?: RequestInit) =>
    Promise.resolve(new Response(JSON.stringify(init?.method === 'POST' ? { status: 'sent' } : {}))),
  );
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
    expect(fetchMock).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Send now' }));
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
    expect(fetchMock).not.toHaveBeenCalled();
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
