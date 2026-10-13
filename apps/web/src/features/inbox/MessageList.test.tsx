import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MessageSummary } from '../../api/types';
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

const page = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });

describe('MessageList', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('loads the next page using next_cursor', async () => {
    const messagePages = [
      page({ items: [msg('1', 'First mail')], next_cursor: 'c1' }),
      page({ items: [msg('2', 'Second mail')], next_cursor: null }),
    ];
    const fetchMock = vi.fn((url: string) => {
      if (url.includes('/labels')) return Promise.resolve(page({ items: [] }));
      return Promise.resolve((url.includes('cursor=c1') ? messagePages[1] : messagePages[0])!.clone());
    });
    vi.stubGlobal('fetch', fetchMock);

    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter>
          <MessageList ws="w1" />
        </MemoryRouter>
      </QueryClientProvider>,
    );

    expect(await screen.findByText('First mail')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Load more' }));
    expect(await screen.findByText('Second mail')).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Load more' })).toBeNull());
    const urls = fetchMock.mock.calls.map((c) => c[0] as string);
    expect(urls.some((u) => u.includes('cursor=c1'))).toBe(true);
  });
});
