import { render, screen, waitFor } from '@testing-library/react';
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

  it('renders sent drafts read-only', () => {
    setup(message({ draft: { ...draft, status: 'sent' } }));
    expect(screen.getByLabelText('Draft reply')).toHaveAttribute('readonly');
    expect(screen.queryByRole('button', { name: 'Send reply…' })).toBeNull();
    expect(screen.getByText(/Sent from Gmail on/)).toBeInTheDocument();
  });
});
