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

  it('creates the forwarding source before sending a test email when none exists', async () => {
    const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
      if (url.endsWith('/sources/forwarding')) return Promise.resolve(new Response(JSON.stringify({ id: 's1' })));
      if (url.endsWith('/test-email')) return Promise.resolve(new Response(JSON.stringify({ status: 'queued' })));
      return Promise.resolve(new Response(JSON.stringify({ items: [], next_cursor: null })));
    });
    vi.stubGlobal('fetch', fetchMock);
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter>
          <GettingStarted ws="w1" />
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await userEvent.click(await screen.findByRole('button', { name: 'Send a test email' }));
    expect(await screen.findByText(/Test email queued/)).toBeInTheDocument();
    const calls = fetchMock.mock.calls.map(([u, i]) => `${i?.method ?? 'GET'} ${u}`);
    const fwd = calls.indexOf('POST /api/v1/workspaces/w1/sources/forwarding');
    const test = calls.indexOf('POST /api/v1/workspaces/w1/sources/s1/test-email');
    expect(fwd).toBeGreaterThan(-1);
    expect(test).toBeGreaterThan(fwd);
  });
});
