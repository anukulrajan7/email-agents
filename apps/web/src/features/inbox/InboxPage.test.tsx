import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { InboxPage } from './InboxPage';

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

function renderInbox(path: string) {
  vi.stubGlobal(
    'fetch',
    vi.fn((url: string) =>
      Promise.resolve(
        url.includes('/messages/m1')
          ? json({ error: { code: 'not_found', message: 'Message not found' } }, 404)
          : json({ items: [], next_cursor: null }),
      ),
    ),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/w/:ws/inbox" element={<InboxPage />} />
          <Route path="/w/:ws/inbox/:messageId" element={<InboxPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe('InboxPage on small screens', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('shows only the list when no message is open', () => {
    renderInbox('/w/w1/inbox');
    expect(screen.getByRole('region', { name: 'Messages' })).not.toHaveClass('hidden');
    expect(screen.getByRole('region', { name: 'Message detail', hidden: true })).toHaveClass('hidden');
    expect(screen.queryByRole('link', { name: /Back to inbox/ })).toBeNull();
  });

  it('stacks to the detail with a back control that returns to the list', async () => {
    renderInbox('/w/w1/inbox/m1');
    const back = screen.getByRole('link', { name: /Back to inbox/ });
    expect(back).toHaveAttribute('href', '/w/w1/inbox');
    expect(screen.getByRole('region', { name: 'Messages', hidden: true })).toHaveClass('hidden');
    expect(screen.getByRole('region', { name: 'Message detail' })).not.toHaveClass('hidden');

    await userEvent.click(back);
    expect(screen.queryByRole('link', { name: /Back to inbox/ })).toBeNull();
    expect(screen.getByRole('region', { name: 'Messages' })).not.toHaveClass('hidden');
  });
});
