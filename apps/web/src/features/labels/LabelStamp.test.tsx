import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Label } from '../../api/types';
import { LabelStamp } from './LabelStamp';

const label: Label = {
  id: 'l1',
  key: 'invoice',
  name: 'Invoices',
  description: 'Invoices and receipts from vendors',
  color: 'plum',
  position: 1,
  system: false,
  archived: false,
  message_count: 3,
};

function setup(key: string) {
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(JSON.stringify({ items: [label] }), { status: 200 }))));
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <LabelStamp ws="w1" category={key} />
    </QueryClientProvider>,
  );
}

describe('LabelStamp', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('renders the label name with the label colour', async () => {
    setup('invoice');
    const stamp = await screen.findByText('Invoices');
    expect(stamp).toHaveClass('text-plum');
  });

  it('falls back to the raw key in muted style for unknown labels', async () => {
    setup('mystery_key');
    const stamp = screen.getByText('mystery_key');
    expect(stamp).toHaveClass('text-ink-muted');
    await screen.findByText('mystery_key');
    expect(screen.queryByText('Invoices')).toBeNull();
  });
});
