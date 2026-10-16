import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { FlagsPanel } from './FlagsPanel';
import type { WorkspaceFlag } from './types';

const flags = (): WorkspaceFlag[] => [
  { key: 'handoff', description: 'Timesheet desk: one owner for each timesheet email.', mode: 'off', default: 'off' },
  { key: 'supervisor', description: 'A supervisor checks each agent proposal.', mode: 'compare', default: 'off' },
];

const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));

function setup(role: 'admin' | 'operator' = 'admin') {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    if (url.endsWith('/me')) {
      return json({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role, created_at: '' }], auth_mode: 'dev' });
    }
    if (url.includes('/flags/') && init?.method === 'PUT') return json({ ...flags()[0], mode: JSON.parse(String(init.body)).mode });
    if (url.endsWith('/flags')) return json({ items: flags() });
    return json({ items: [] });
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/ai-health']}>
        <Routes>
          <Route path="/w/:ws/ai-health" element={<FlagsPanel />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('FlagsPanel', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('lists each flag with its description and the three states, the current one pressed', async () => {
    setup();

    expect(await screen.findByText('Timesheet desk: one owner for each timesheet email.')).toBeInTheDocument();
    const handoff = screen.getByRole('group', { name: 'handoff mode' });
    expect(within(handoff).getAllByRole('button').map((b) => b.textContent)).toEqual(['Off', 'Compare', 'On']);
    expect(within(handoff).getByRole('button', { name: 'Off' })).toHaveAttribute('aria-pressed', 'true');
    const supervisor = screen.getByRole('group', { name: 'supervisor mode' });
    expect(within(supervisor).getByRole('button', { name: 'Compare' })).toHaveAttribute('aria-pressed', 'true');
    expect(within(supervisor).getByRole('button', { name: 'On' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('sets a flag when a state is chosen', async () => {
    const fetchMock = setup();

    await userEvent.click(within(await screen.findByRole('group', { name: 'handoff mode' })).getByRole('button', { name: 'On' }));

    await waitFor(() =>
      expect(fetchMock.mock.calls.some(([url, init]) => String(url).endsWith('/workspaces/w1/flags/handoff') && init?.method === 'PUT' && init.body === JSON.stringify({ mode: 'on' }))).toBe(true),
    );
  });

  it('does not set a flag when its current state is chosen again', async () => {
    const fetchMock = setup();

    await userEvent.click(within(await screen.findByRole('group', { name: 'handoff mode' })).getByRole('button', { name: 'Off' }));

    expect(fetchMock.mock.calls.some(([, init]) => init?.method === 'PUT')).toBe(false);
  });

  it('shows nothing and asks for nothing when the person is not an admin', async () => {
    const fetchMock = setup('operator');

    await waitFor(() => expect(fetchMock.mock.calls.some(([url]) => String(url).endsWith('/me'))).toBe(true));
    await waitFor(() => expect(screen.queryByText('New behaviour')).not.toBeInTheDocument());
    expect(screen.queryByRole('group')).not.toBeInTheDocument();
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes('/flags'))).toBe(false);
  });
});
