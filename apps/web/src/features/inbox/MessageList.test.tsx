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
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

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

  function renderList(summaries: MessageSummary[]) {
    const fetchMock = vi.fn((url: string) =>
      Promise.resolve(url.includes('/labels') ? page({ items: [] }) : page({ items: summaries, next_cursor: null })),
    );
    vi.stubGlobal('fetch', fetchMock);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter>
          <MessageList ws="w1" />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    return fetchMock;
  }

  const messageUrls = (fetchMock: ReturnType<typeof vi.fn>) =>
    fetchMock.mock.calls.map((c) => c[0] as string).filter((u) => u.includes('/messages'));

  it('requests importance order by default and persists the Newest choice', async () => {
    const fetchMock = renderList([msg('1', 'First mail')]);
    await screen.findByText('First mail');
    expect(messageUrls(fetchMock).every((u) => u.includes('sort=importance'))).toBe(true);
    expect(screen.getByRole('button', { name: 'Important first' })).toHaveAttribute('aria-pressed', 'true');

    await userEvent.click(screen.getByRole('button', { name: 'Newest' }));
    await waitFor(() => expect(messageUrls(fetchMock).some((u) => u.includes('sort=recent'))).toBe(true));
    expect(localStorage.getItem('inbox.sort')).toBe('recent');
    expect(screen.getByRole('button', { name: 'Newest' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('filters by reply status from the label menu', async () => {
    const fetchMock = renderList([msg('1', 'First mail')]);
    await screen.findByText('First mail');
    await userEvent.selectOptions(screen.getByLabelText('Filter by label'), 'Replied');
    await waitFor(() => expect(messageUrls(fetchMock).some((u) => u.includes('reply=replied') && !u.includes('category='))).toBe(true));
  });

  it('starts from the stored sort preference', async () => {
    localStorage.setItem('inbox.sort', 'recent');
    const fetchMock = renderList([msg('1', 'First mail')]);
    await screen.findByText('First mail');
    expect(messageUrls(fetchMock)[0]).toContain('sort=recent');
  });

  it('shows sender, subject with summary, needs-reply chip, high-priority marker and unread weight', async () => {
    renderList([
      {
        ...msg('1', 'Invoice 123'),
        from: 'Ada Lovelace <ada@x.test>',
        priority: 'high',
        needs_reply: true,
        summary: 'Wants approval by Friday',
        unread: true,
        needs_review: true,
      },
    ]);
    expect(await screen.findByText('Ada Lovelace')).toBeInTheDocument();
    expect(screen.getByText('Invoice 123')).toHaveClass('font-bold');
    expect(screen.getByText(/Wants approval by Friday/)).toBeInTheDocument();
    expect(screen.getByText('Needs reply', { selector: 'span' })).toBeInTheDocument();
    expect(screen.getByRole('img', { name: 'High priority' })).toBeInTheDocument();
    expect(screen.queryByText('Needs review')).toBeNull();
  });
});
