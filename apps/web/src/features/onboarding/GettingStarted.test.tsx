import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GettingStarted } from './GettingStarted';

function renderIt() {
  vi.stubGlobal('fetch', vi.fn(() => Promise.resolve(new Response(JSON.stringify({ items: [], next_cursor: null })))));
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter>
        <GettingStarted ws="w1" />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe('GettingStarted', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.unstubAllGlobals());

  it('hides after dismiss and stays hidden on remount', async () => {
    renderIt();
    expect(await screen.findByRole('heading', { name: 'Getting started' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByRole('heading', { name: 'Getting started' })).toBeNull();
    expect(localStorage.getItem('getting-started:w1:dismissed')).toBe('1');
  });
});
