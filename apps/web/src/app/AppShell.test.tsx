import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { TOKEN_KEY } from '../api/client';
import { AppShell } from './AppShell';

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });

function renderShell(role: 'admin' | 'member', packs?: string[]) {
  vi.stubGlobal(
    'fetch',
    vi.fn((url: string) =>
      Promise.resolve(
        url.endsWith('/me')
          ? json({
              user: { id: 'u1', email: 'priya@acme.test', name: 'Priya Sharma' },
              workspaces: [{ id: 'w1', name: 'Acme Talent', role, created_at: '', ...(packs ? { packs } : {}) }],
              auth_mode: 'password',
            })
          : json({ items: [], pending_count: 0, open_count: 0 }),
      ),
    ),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/inbox']}>
        <Routes>
          <Route path="/w/:ws" element={<AppShell />}>
            <Route path="inbox" element={<p>Inbox page</p>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

const itemsOf = (nav: HTMLElement, group: string) =>
  within(within(nav).getByRole('list', { name: group }))
    .getAllByRole('link')
    .map((a) => a.textContent);

describe('AppShell navigation', () => {
  beforeEach(() => localStorage.setItem(TOKEN_KEY, 'tok'));
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('groups the sidebar into Work, Teach and Settings without admin-only items for members', async () => {
    renderShell('member');
    const nav = await screen.findByRole('navigation', { name: 'Main' });
    expect(itemsOf(nav, 'Work')).toEqual(['Inbox', 'Agents', 'Your team', 'Results', 'Alerts']);
    expect(itemsOf(nav, 'HRMS')).toEqual(['People', 'Performance', 'Timesheets', 'Portal reconciliation', 'Escalations', 'Matching']);
    expect(itemsOf(nav, 'Teach')).toEqual(['Labels', 'Memory', 'Automations']);
    expect(itemsOf(nav, 'Settings')).toEqual(['Integrations', 'Workflows', 'Reviews']);
    expect(itemsOf(nav, 'Help')).toEqual(['Guide']);
    for (const link of screen.getAllByRole('link', { name: 'Open the guide' })) expect(link).toHaveAttribute('href', '/w/w1/guide');
    expect(within(nav).getByRole('link', { name: 'Inbox' })).toHaveAttribute('aria-current', 'page');
    expect(within(nav).getByRole('link', { name: 'Labels' })).toHaveAttribute('href', '/w/w1/labels');
  });

  it('shows a generic Team group, with no HR modules, when the HR & recruiting pack is off', async () => {
    renderShell('member', ['general']);
    const nav = await screen.findByRole('navigation', { name: 'Main' });
    expect(itemsOf(nav, 'Team')).toEqual(['People', 'Performance', 'Escalations']);
    expect(within(nav).queryByRole('list', { name: 'HRMS' })).toBeNull();
    expect(within(nav).queryByRole('link', { name: 'Matching' })).toBeNull();
    expect(within(nav).queryByRole('link', { name: 'Timesheets' })).toBeNull();
  });

  it('puts every HRMS module in one group, with Matching moved under it, when the pack is on', async () => {
    renderShell('member', ['general', 'staffing']);
    const nav = await screen.findByRole('navigation', { name: 'Main' });
    expect(itemsOf(nav, 'HRMS')).toEqual(['People', 'Performance', 'Timesheets', 'Portal reconciliation', 'Escalations', 'Matching']);
    expect(within(nav).getByRole('link', { name: 'Portal reconciliation' })).toHaveAttribute('href', '/w/w1/portal');
    expect(within(nav).queryByRole('list', { name: 'Team' })).toBeNull();
  });

  it('adds Jobs and AI usage to Settings for admins', async () => {
    renderShell('admin');
    const nav = await screen.findByRole('navigation', { name: 'Main' });
    expect(itemsOf(nav, 'Settings')).toEqual(['Integrations', 'Workflows', 'Packs', 'Reviews', 'Jobs', 'AI usage']);
    expect(within(nav).getByRole('link', { name: 'AI usage' })).toHaveAttribute('href', '/w/w1/usage');
  });

  it('shows the workspace and signed-in user with sign out', async () => {
    renderShell('member');
    expect(await screen.findAllByText('Acme Talent')).not.toHaveLength(0);
    expect(screen.getAllByText('Priya Sharma')).not.toHaveLength(0);
    expect(screen.getAllByText('priya@acme.test')).not.toHaveLength(0);
    expect(screen.getAllByRole('button', { name: 'Sign out' })).not.toHaveLength(0);
  });

  it('opens the same grouped navigation from the mobile menu', async () => {
    renderShell('member');
    await userEvent.click(await screen.findByRole('button', { name: 'Menu' }));
    const drawer = screen.getByRole('dialog', { name: 'Menu' });
    const nav = within(drawer).getByRole('navigation', { name: 'Main' });
    expect(itemsOf(nav, 'Teach')).toEqual(['Labels', 'Memory', 'Automations']);
    await userEvent.click(within(drawer).getByRole('button', { name: 'Close menu' }));
    expect(screen.queryByRole('dialog', { name: 'Menu' })).toBeNull();
  });
});
