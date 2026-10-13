import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MemoryPage } from './MemoryPage';
import type { LearnedMemory } from './types';

const mem = (over: Partial<LearnedMemory>): LearnedMemory => ({
  id: 'm1',
  scope: 'workspace',
  contact_email: null,
  kind: 'fact',
  content: 'hello',
  status: 'active',
  source: 'user',
  created_by: null,
  evidence_message_id: null,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
  ...over,
});

const auto = (over: Partial<LearnedMemory>) =>
  mem({ source: 'learned', origin: 'auto', confidence: 0.9, use_count: 0, pinned: false, source_message_id: null, ...over });

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });

function setup(items: LearnedMemory[], extra?: (url: string, init?: RequestInit) => unknown) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    const custom = extra?.(url, init);
    if (custom !== undefined) return Promise.resolve(json(custom));
    if (url.includes('/memories?status=active')) return Promise.resolve(json({ items }));
    return Promise.resolve(json({ items: [] }));
  });
  vi.stubGlobal('fetch', fetchMock);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/memory']}>
        <Routes>
          <Route path="/w/:ws/memory" element={<MemoryPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return fetchMock;
}

const patchCall = (fetchMock: ReturnType<typeof setup>) => fetchMock.mock.calls.find(([, init]) => init?.method === 'PATCH');

describe('MemoryPage', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('groups memories by kind under their own headings', async () => {
    setup([
      auto({ id: 'a', scope: 'contact', contact_email: 'sam@acme.com', content: 'Sam prefers short replies' }),
      auto({ id: 'b', kind: 'client' as LearnedMemory['kind'], content: 'Acme pays net 30' }),
      auto({ id: 'c', kind: 'fact', content: 'Office closes at 6pm' }),
      auto({ id: 'd', kind: 'preference', content: 'Sign off with Best' }),
    ]);

    const contacts = await screen.findByRole('region', { name: 'Contacts' });
    expect(within(contacts).getByText('Sam prefers short replies')).toBeInTheDocument();
    expect(within(screen.getByRole('region', { name: 'Clients' })).getByText('Acme pays net 30')).toBeInTheDocument();
    expect(within(screen.getByRole('region', { name: 'Standing answers' })).getByText('Office closes at 6pm')).toBeInTheDocument();
    expect(within(screen.getByRole('region', { name: 'Team preferences' })).getByText('Sign off with Best')).toBeInTheDocument();
  });

  it('shows confidence, usage and a link to the source email subject', async () => {
    setup([auto({ id: 'a', confidence: 0.7, use_count: 3, source_message_id: 'e1', content: 'Net 30' })], (url) =>
      url.includes('/messages/e1') ? { id: 'e1', subject: 'Re: Invoice 42' } : undefined,
    );

    expect(await screen.findByText('Likely')).toBeInTheDocument();
    expect(screen.getByText('Used 3 times')).toBeInTheDocument();
    const link = await screen.findByRole('link', { name: 'Re: Invoice 42' });
    expect(link).toHaveAttribute('href', '/w/w1/inbox/e1');
  });

  it('confirm on an unsure memory PATCHes confirmed true', async () => {
    const fetchMock = setup([auto({ id: 'u1', confidence: 0.4, content: 'Maybe net 45' }), auto({ id: 's1', confidence: 0.95, content: 'Sure thing' })]);

    expect(await screen.findByText('Unsure')).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Confirm' })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: 'Confirm' }));

    await waitFor(() => {
      const call = patchCall(fetchMock);
      expect(call?.[0]).toContain('/workspaces/w1/memories/u1');
      expect(JSON.parse(call?.[1]?.body as string)).toEqual({ confirmed: true });
    });
  });

  it('pin PATCHes pinned true and unpin sends false', async () => {
    const fetchMock = setup([auto({ id: 'p1', content: 'Pin me' }), auto({ id: 'p2', pinned: true, content: 'Already pinned' })]);

    const pin = await screen.findByRole('button', { name: 'Pin' });
    await userEvent.click(pin);
    await waitFor(() => {
      const call = patchCall(fetchMock);
      expect(call?.[0]).toContain('/memories/p1');
      expect(JSON.parse(call?.[1]?.body as string)).toEqual({ pinned: true });
    });

    await userEvent.click(screen.getByRole('button', { name: 'Unpin' }));
    await waitFor(() => {
      const bodies = fetchMock.mock.calls.filter(([, init]) => init?.method === 'PATCH').map(([, init]) => JSON.parse(init?.body as string));
      expect(bodies).toContainEqual({ pinned: false });
    });
  });

  it('delete archives the memory', async () => {
    const fetchMock = setup([auto({ id: 'd1', content: 'Wrong fact' })]);

    await userEvent.click(await screen.findByRole('button', { name: 'Delete' }));
    await waitFor(() => {
      const call = patchCall(fetchMock);
      expect(call?.[0]).toContain('/memories/d1');
      expect(JSON.parse(call?.[1]?.body as string)).toEqual({ status: 'archived' });
    });
  });

  it('filters between learned automatically and added by you', async () => {
    setup([auto({ id: 'a', content: 'Learned fact' }), mem({ id: 'b', origin: 'manual', content: 'Typed fact' })]);

    expect(await screen.findByText('Learned fact')).toBeInTheDocument();
    expect(screen.getByText('Typed fact')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: /Added by you/ }));
    expect(screen.queryByText('Learned fact')).not.toBeInTheDocument();
    expect(screen.getByText('Typed fact')).toBeInTheDocument();

    await userEvent.click(screen.getByRole('tab', { name: /Learned automatically/ }));
    expect(screen.getByText('Learned fact')).toBeInTheDocument();
    expect(screen.queryByText('Typed fact')).not.toBeInTheDocument();
  });

  it('explains that memory learns automatically when empty', async () => {
    setup([]);
    expect(await screen.findByText('Nothing learned yet')).toBeInTheDocument();
    expect(screen.getByText(/learns from your emails and from the replies/)).toBeInTheDocument();
  });

  it('renders memory content as plain text', async () => {
    const evil = '<img src=x onerror=alert(1)><b>bold</b>';
    setup([auto({ content: evil })]);

    expect(await screen.findByText(evil)).toBeInTheDocument();
    expect(document.querySelector('img')).toBeNull();
    expect(document.querySelector('b')).toBeNull();
  });

  it('teach a fact posts a manual workspace fact', async () => {
    const fetchMock = setup([]);

    await userEvent.click(await screen.findByText('Teach a fact'));
    await userEvent.type(screen.getByLabelText('Something the assistant should know'), 'We close on Fridays');
    await userEvent.click(screen.getByRole('button', { name: 'Add fact' }));

    await waitFor(() => {
      const call = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
      expect(call?.[0]).toContain('/workspaces/w1/memories');
      expect(JSON.parse(call?.[1]?.body as string)).toEqual({ scope: 'workspace', kind: 'fact', content: 'We close on Fridays' });
    });
  });
});
