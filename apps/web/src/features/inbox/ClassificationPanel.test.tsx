import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Classification } from '../../api/types';
import { ClassificationPanel } from './ClassificationPanel';

const cls: Classification = {
  category: 'leave',
  priority: 'normal',
  requires_reply: false,
  confidence: 0.5,
  needs_review: true,
  source: 'llm',
  model_id: 'm',
  updated_at: 't1',
};

describe('ClassificationPanel', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('approves via the endpoint and keeps the correction form collapsed', async () => {
    const fetchMock = vi.fn((_url: string, init?: RequestInit) =>
      Promise.resolve(new Response(JSON.stringify(init?.method === 'POST' ? { ...cls, needs_review: false } : { items: [] }))),
    );
    vi.stubGlobal('fetch', fetchMock);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ClassificationPanel ws="w1" messageId="m1" classification={cls} />
      </QueryClientProvider>,
    );
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
    await userEvent.click(screen.getByRole('button', { name: 'Approve label' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/messages/m1/classification/approve',
        expect.objectContaining({ method: 'POST' }),
      ),
    );
    expect(await screen.findByText('Approved ✓')).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Change label' }));
    expect(screen.getByRole('button', { name: 'Save' })).toBeInTheDocument();
  });
});
