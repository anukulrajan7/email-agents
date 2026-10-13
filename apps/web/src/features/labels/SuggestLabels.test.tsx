import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SuggestLabels, nearestLabelColor } from './SuggestLabels';

const items = [
  { key: 'invoice', name: 'Invoices', description: 'Vendor invoices and payment requests', color: '#2F6F4E', example_subjects: ['Invoice #123 for September'], estimated_count: 12 },
  { key: 'events', name: 'Events', description: 'Invitations to meetups and webinars', color: '#6b3f6e', example_subjects: [], estimated_count: 4 },
];

const json = (body: unknown, status = 200) => Promise.resolve(new Response(JSON.stringify(body), { status }));

function setup(suggest: () => Promise<Response>) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    if (url.endsWith('/labels/suggest')) return suggest();
    if (init?.method === 'POST') return json({ id: 'new' }, 201);
    return json({ items: [] });
  });
  vi.stubGlobal('fetch', fetchMock);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <SuggestLabels ws="w1" />
    </QueryClientProvider>,
  );
  return { fetchMock, user: userEvent.setup() };
}

describe('SuggestLabels', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('maps suggestion hex colours to the nearest palette colour', () => {
    expect(nearestLabelColor('#2F6F4E')).toBe('sage');
    expect(nearestLabelColor('plum')).toBe('plum');
    expect(nearestLabelColor('nonsense')).toBeUndefined();
  });

  it('shows suggestions, adds one with POST /labels and dismisses another', async () => {
    const { fetchMock, user } = setup(() => json({ items }));
    await user.click(screen.getByRole('button', { name: 'Suggest labels from my email' }));
    expect(await screen.findByText('Vendor invoices and payment requests')).toBeInTheDocument();
    expect(screen.getByText('Invoice #123 for September')).toBeInTheDocument();
    expect(screen.getByText('~12 emails')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Add Invoices' }));
    await waitFor(() => expect(screen.queryByText('Vendor invoices and payment requests')).toBeNull());
    const post = fetchMock.mock.calls.find(([u, i]) => u.endsWith('/workspaces/w1/labels') && i?.method === 'POST');
    expect(JSON.parse(post?.[1]?.body as string)).toEqual({
      name: 'Invoices',
      description: 'Vendor invoices and payment requests',
      color: 'sage',
    });

    await user.click(screen.getByRole('button', { name: 'Dismiss Events' }));
    expect(screen.queryByText('Invitations to meetups and webinars')).toBeNull();
    expect(screen.getByRole('status')).toHaveTextContent('All suggestions handled');
  });

  it('shows a friendly retry when suggestions are unavailable', async () => {
    let calls = 0;
    const { user } = setup(() => {
      calls += 1;
      return calls === 1
        ? json({ error: { code: 'ai_unavailable', message: 'upstream down', request_id: 'r' } }, 503)
        : json({ items });
    });
    await user.click(screen.getByRole('button', { name: 'Suggest labels from my email' }));
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('please try again in a moment');
    expect(alert).not.toHaveTextContent('upstream down');
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByText('Vendor invoices and payment requests')).toBeInTheDocument();
  });
});
