import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Memory } from '../../api/types';
import { MemoryPage } from './MemoryPage';

const mem = (over: Partial<Memory>): Memory => ({
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

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200 });

function setup(handler: (url: string, init?: RequestInit) => unknown) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => Promise.resolve(json(handler(url, init))));
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

describe('MemoryPage', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('accepting a suggestion PATCHes status active', async () => {
    const suggested = mem({ id: 's1', status: 'suggested', source: 'learned', content: 'Sign off with Best', evidence_message_id: 'e1' });
    const fetchMock = setup((url, init) => {
      if (init?.method === 'PATCH') return { ...suggested, status: 'active' };
      if (url.includes('/memories') && url.includes('status=suggested')) return { items: [suggested] };
      return { items: [] };
    });

    await userEvent.click(await screen.findByRole('button', { name: 'Accept' }));
    await waitFor(() => {
      const call = fetchMock.mock.calls.find(([, init]) => init?.method === 'PATCH');
      expect(call?.[0]).toContain('/workspaces/w1/memories/s1');
      expect(JSON.parse(call?.[1]?.body as string)).toEqual({ status: 'active' });
    });
    expect(screen.getByRole('link', { name: 'Learned from' })).toHaveAttribute('href', '/w/w1/inbox/e1');
  });

  it('renders memory content as plain text', async () => {
    const evil = '<img src=x onerror=alert(1)><b>bold</b>';
    setup((url) => (url.includes('scope=workspace') ? { items: [mem({ content: evil })] } : { items: [] }));

    expect(await screen.findByText(evil)).toBeInTheDocument();
    expect(document.querySelector('img')).toBeNull();
    expect(document.querySelector('b')).toBeNull();
  });

  it('disables submit when content exceeds 500 characters', async () => {
    setup(() => ({ items: [] }));

    const box = await screen.findByLabelText('Note');
    const submit = screen.getByRole('button', { name: 'Add note' });
    expect(submit).toBeDisabled();
    await userEvent.click(box);
    await userEvent.paste('a'.repeat(500));
    expect(submit).toBeEnabled();
    await userEvent.paste('a');
    expect(submit).toBeDisabled();
    expect(screen.getByText('501/500')).toBeInTheDocument();
  });
});
