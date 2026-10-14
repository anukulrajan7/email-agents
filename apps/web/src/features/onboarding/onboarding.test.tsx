import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TOKEN_KEY } from '../../api/client';
import { agent, json, meBody, registry } from '../../test/fixtures';
import { GoalWizard } from './GoalWizard';
import { WelcomePage } from './WelcomePage';
import { suggestPacks } from './goals';

const packs = ['general', 'support', 'finance', 'staffing'].map((key) => ({ key, name: `${key} pack`, description: `${key} desc`, labels: [], presets: [], features: [] }));

function wrap(ui: React.ReactNode, path: string, initial: string) {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[initial]}>
        <Routes>
          <Route path={path} element={ui} />
          <Route path="/w/:ws/welcome" element={<p>Welcome page reached</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe('suggestPacks', () => {
  it('always includes general and adds packs for the words used', () => {
    expect(suggestPacks('')).toEqual(['general']);
    expect(suggestPacks('Chase invoices and answer support tickets')).toEqual(['general', 'support', 'finance']);
    expect(suggestPacks('We do HR and recruiting')).toEqual(['general', 'staffing']);
  });
});

describe('GoalWizard', () => {
  beforeEach(() => localStorage.setItem(TOKEN_KEY, 'tok'));
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('goes goal, team, name and creates the workspace with the goal and packs', async () => {
    const calls: { url: string; method: string; body?: string }[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string, init?: RequestInit) => {
        calls.push({ url, method: init?.method ?? 'GET', body: init?.body as string | undefined });
        if (url.endsWith('/packs')) return Promise.resolve(json({ items: packs }));
        if (init?.method === 'POST') return Promise.resolve(json({ id: 'w9', name: 'Acme', role: 'admin', created_at: '' }));
        return Promise.resolve(json(meBody()));
      }),
    );
    const user = userEvent.setup();
    wrap(<GoalWizard />, '/workspaces', '/workspaces');

    expect(screen.getByRole('heading', { name: 'What do you want help with?' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Chase invoices and payments' }));
    expect(screen.getByRole('button', { name: 'Chase invoices and payments' })).toHaveAttribute('aria-pressed', 'true');
    await user.type(screen.getByLabelText(/In your own words/), ' and HR leave');
    await user.click(screen.getByRole('button', { name: 'Continue' }));

    expect(await screen.findByRole('heading', { name: 'Here is your team' })).toBeInTheDocument();
    expect(await screen.findByRole('checkbox', { name: /finance pack/ })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: /staffing pack/ })).toBeChecked();
    expect(screen.getByRole('list', { name: 'Agents you will get' })).toHaveTextContent('Invoices & Payments');
    await user.click(screen.getByRole('button', { name: 'Continue' }));

    await user.type(screen.getByLabelText('Workspace name'), 'Acme');
    await user.click(screen.getByRole('button', { name: 'Create workspace' }));
    await waitFor(() => expect(screen.getByText('Welcome page reached')).toBeInTheDocument());
    const post = calls.find((c) => c.method === 'POST');
    expect(JSON.parse(post?.body ?? '{}')).toEqual({ name: 'Acme', packs: ['general', 'finance', 'staffing'], goal: 'Chase invoices and payments and HR leave' });
  });
});

describe('WelcomePage', () => {
  beforeEach(() => localStorage.setItem(TOKEN_KEY, 'tok'));
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('connects mail, introduces the agents with avatars and ends on a first win', async () => {
    const mine = agent({ id: 'a-me', name: 'Asha’s agent', category: 'personal', scope: 'personal', avatar: '🌼' });
    const meera = agent({ id: 'a-l', name: 'Meera', category: 'general', kind: 'lead' });
    const calls: string[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string, init?: RequestInit) => {
        calls.push(`${init?.method ?? 'GET'} ${url}`);
        if (url.endsWith('/me')) return Promise.resolve(json(meBody('admin', { goal: 'Chase invoices' })));
        if (url.endsWith('/sources')) return Promise.resolve(json({ items: [{ id: 's1', kind: 'nango_gmail', status: 'connected' }] }));
        if (url.endsWith('/agents')) return Promise.resolve(json(registry([meera], mine)));
        if (url.includes('/messages')) return Promise.resolve(json({ items: [{ id: 'm1' }, { id: 'm2' }], next_cursor: null }));
        return Promise.resolve(json({}));
      }),
    );
    const user = userEvent.setup();
    wrap(<WelcomePage />, '/w/:ws/welcome', '/w/w1/welcome');

    expect(await screen.findByText('Your goal: Chase invoices')).toBeInTheDocument();
    expect(screen.getByRole('listitem', { current: 'step' })).toHaveTextContent('1. Connect your mail');
    await user.click(await screen.findByRole('button', { name: 'Continue' }));

    expect(await screen.findByRole('radio', { name: 'Panda' })).toBeInTheDocument();
    expect(screen.getByText('Meera')).toBeInTheDocument();
    await user.click(screen.getByRole('radio', { name: 'Owl' }));
    await waitFor(() => expect(calls).toContain('PATCH /api/v1/workspaces/w1/agents/a-me'));
    await user.click(screen.getByRole('button', { name: 'Looks good' }));

    expect(await screen.findByText(/Meera has already read 2 emails/)).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'See your team' })).toHaveAttribute('href', '/w/w1/agent-team');
  });
});
