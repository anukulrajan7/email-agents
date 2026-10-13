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
  model_id: 'qwen/qwen3-secret-model',
  updated_at: 't1',
};

const labels = {
  items: [
    { id: 'l1', key: 'leave', name: 'Leave', description: 'Leave requests', color: 'sage', position: 0, system: false, archived: false, message_count: 1 },
    { id: 'l2', key: 'invoice', name: 'Invoice', description: 'Invoices', color: 'plum', position: 1, system: false, archived: false, message_count: 1 },
  ],
};

function setup() {
  const fetchMock = vi.fn((url: string, init?: RequestInit) =>
    Promise.resolve(new Response(JSON.stringify(init?.method === 'PATCH' ? { ...cls, category: 'invoice' } : url.includes('/labels') ? labels : {}))),
  );
  vi.stubGlobal('fetch', fetchMock);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ClassificationPanel ws="w1" messageId="m1" classification={cls} />
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('ClassificationPanel', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('has no approve flow and never shows the model id', () => {
    setup();
    expect(screen.queryByRole('button', { name: /approve/i })).toBeNull();
    expect(screen.queryByText(/needs review/i)).toBeNull();
    expect(document.body.textContent).not.toMatch(/qwen|model/i);
    expect(screen.getByText(/automatically/)).toBeInTheDocument();
  });

  it('changes the label in one click via the correction endpoint', async () => {
    const fetchMock = setup();
    const select = await screen.findByRole('combobox', { name: 'Change label' });
    await screen.findByRole('option', { name: 'Invoice' });
    await userEvent.selectOptions(select, 'invoice');
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/messages/m1/classification',
        expect.objectContaining({ method: 'PATCH', body: JSON.stringify({ category: 'invoice', priority: 'normal' }) }),
      ),
    );
    expect(await screen.findByText('Saved')).toBeInTheDocument();
  });
});
