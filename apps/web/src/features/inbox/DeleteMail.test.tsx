import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MessageSummary } from '../../api/types';
import { ToastProvider } from '../../components/ui';
import { DeleteEmailButton } from './DeleteEmail';
import { MessageList } from './MessageList';

const msg = (id: string, subject: string): MessageSummary => ({
  id,
  subject,
  from: 'a@b.test',
  original_from: null,
  received_at: new Date().toISOString(),
  processing_status: 'classified',
  category: 'leave',
  priority: 'normal',
  needs_review: false,
  attachment_count: 0,
});

const ok = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });

function wrap(children: React.ReactNode) {
  return (
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ToastProvider>
        <MemoryRouter>{children}</MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>
  );
}

type Call = [string, RequestInit];
const deletes = (m: ReturnType<typeof vi.fn>) => (m.mock.calls as unknown as Call[]).filter((c) => !c[0].endsWith('/delete-preview'));

describe('deleting mail', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('deletes a selection from the inbox after a confirm that says the Gmail copy stays', async () => {
    const fetchMock = vi.fn((url: string, init?: RequestInit) => {
      if (url.includes('/labels')) return Promise.resolve(ok({ items: [] }));
      if (url.endsWith('/messages/delete')) return Promise.resolve(ok({ deleted: 2 }));
      void init;
      return Promise.resolve(ok({ items: [msg('1', 'First mail'), msg('2', 'Second mail'), msg('3', 'Third mail')], next_cursor: null }));
    });
    vi.stubGlobal('fetch', fetchMock);
    render(wrap(<MessageList ws="w1" />));
    await screen.findByText('First mail');

    await userEvent.click(screen.getByRole('button', { name: 'Select' }));
    expect(screen.getByRole('button', { name: 'Delete' })).toBeDisabled();
    await userEvent.click(screen.getByRole('checkbox', { name: 'Select First mail' }));
    await userEvent.click(screen.getByRole('checkbox', { name: 'Select Third mail' }));
    expect(screen.getByText('2 selected')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    const dialog = screen.getByRole('dialog', { name: 'Delete from Post Room?' });
    expect(within(dialog).getByText(/Your Gmail copy stays/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Delete 2 emails' }));

    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    const call = fetchMock.mock.calls.find((c) => (c[0] as string).endsWith('/messages/delete'));
    expect(call?.[1]?.method).toBe('POST');
    expect(JSON.parse(String(call?.[1]?.body))).toEqual({ ids: ['1', '3'] });
    expect(await screen.findByText(/2 emails deleted from Post Room/)).toBeInTheDocument();
    expect(screen.queryByRole('checkbox')).toBeNull();
  });

  it('deletes the whole conversation from the reader when asked', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(ok({ deleted: 3 })));
    vi.stubGlobal('fetch', fetchMock);
    render(wrap(<DeleteEmailButton ws="w1" messageId="m9" />));

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    const dialog = screen.getByRole('dialog', { name: 'Delete from Post Room?' });
    await userEvent.click(within(dialog).getByRole('checkbox', { name: 'Also delete the rest of this conversation' }));
    await userEvent.click(within(dialog).getByRole('button', { name: 'Delete' }));

    await waitFor(() => expect(deletes(fetchMock)).toHaveLength(1));
    const [url, init] = deletes(fetchMock)[0]!;
    expect(url).toBe('/api/v1/workspaces/w1/messages/m9/thread');
    expect(init.method).toBe('DELETE');
    expect(await screen.findByText(/3 emails deleted from Post Room/)).toBeInTheDocument();
  });

  it('deletes just this email by default and lets the person back out', async () => {
    const fetchMock = vi.fn(() => Promise.resolve(ok({ deleted: 1 })));
    vi.stubGlobal('fetch', fetchMock);
    render(wrap(<DeleteEmailButton ws="w1" messageId="m9" />));

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(deletes(fetchMock)).toHaveLength(0);

    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    await userEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(deletes(fetchMock)).toHaveLength(1));
    const [url, init] = deletes(fetchMock)[0]!;
    expect(url).toBe('/api/v1/workspaces/w1/messages/delete');
    expect(JSON.parse(String(init.body))).toEqual({ ids: ['m9'] });
  });

  it('warns when a scheduled reply will be cancelled', async () => {
    vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(ok({ emails: 1, scheduled_replies: 1 }))));
    render(wrap(<DeleteEmailButton ws="w1" messageId="m9" />));
    await userEvent.click(screen.getByRole('button', { name: 'Delete' }));
    expect(await screen.findByText('1 scheduled reply will be cancelled.')).toBeInTheDocument();
  });
});
