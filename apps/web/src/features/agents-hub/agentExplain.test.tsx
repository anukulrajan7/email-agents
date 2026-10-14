import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TOKEN_KEY } from '../../api/client';
import type { AgentSummary, Autonomy } from '../../api/types';
import { ToastProvider } from '../../components/ui';
import { agent, json, meBody, registry } from '../../test/fixtures';
import { AgentCard } from './AgentCard';
import { AUTONOMY_PLAIN, agentCopy, agentDisplayName } from './agentCopy';
import { AgentsHubPage } from './AgentsHubPage';

const DESKS: [string, string, AgentSummary['category']][] = [
  ['personal.mailbox', 'Meera', 'personal'],
  ['hrms.leave', 'Ananya', 'hrms'],
  ['hrms.timesheets', 'Rohan', 'hrms'],
  ['hrms.onboarding', 'Sneha', 'hrms'],
  ['hrms.requirements', 'Arjun', 'hrms'],
  ['hrms.payroll', 'Aditya', 'hrms'],
  ['support.resolver', 'Ishaan', 'support'],
  ['sales.qualifier', 'Kabir', 'sales'],
  ['finance.invoices', 'Vikram', 'finance'],
  ['personal.scheduling', 'Nisha', 'personal'],
];

const card = (a: AgentSummary) =>
  render(
    <AgentCard agent={a} index={0} isAdmin busy={false} onToggle={() => undefined} onOpen={() => undefined} />,
  );

describe('agent explanation on every card', () => {
  afterEach(() => vi.unstubAllGlobals());

  it.each(DESKS)('%s: says what it does, when it acts, what it does alone vs asks, and an example', async (template, name, category) => {
    const user = userEvent.setup();
    const level: Autonomy = 1;
    const a = agent({
      id: template,
      name,
      category,
      template_key: template,
      kind: 'system',
      scope: template === 'personal.mailbox' ? 'personal' : 'workspace',
      autonomy: { earned: level, cap: 3, effective: level, limited_by: 'earned' },
    });
    card(a);
    const copy = agentCopy(a);
    expect(screen.queryByText('What it does')).not.toBeInTheDocument();
    const how = screen.getByRole('button', { name: 'How I work' });
    expect(how).toHaveAttribute('aria-expanded', 'false');
    await user.click(how);
    expect(how).toHaveAttribute('aria-expanded', 'true');
    const article = screen.getByRole('article', { name: agentDisplayName(a) });
    for (const label of ['What it does', 'When it acts', 'On its own', 'Asks you', 'For example']) expect(within(article).getByText(label)).toBeInTheDocument();
    expect(within(article).getByText(copy.does)).toBeInTheDocument();
    expect(within(article).getByText(copy.when)).toBeInTheDocument();
    expect(within(article).getByText(copy.example)).toBeInTheDocument();
    expect(within(article).getByText(AUTONOMY_PLAIN[level].headline)).toBeInTheDocument();
    expect(within(article).getByText(copy.role)).toBeInTheDocument();
    expect(article.textContent).not.toMatch(/\b[a-z]+\.[a-z]+\b.*template|My assistant|\(personal\)|gpt|claude|gemini|openai|anthropic/i);
  });

  it.each([0, 1, 2, 3] as Autonomy[])('level %s has its own plain-English line', async (level) => {
    const user = userEvent.setup();
    card(agent({ id: 'x', name: 'Kabir', category: 'sales', template_key: 'sales.qualifier', autonomy: { earned: level, cap: 3, effective: level, limited_by: 'earned' } }));
    expect(screen.getByText(AUTONOMY_PLAIN[level].headline)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'How I work' }));
    expect(screen.getByText(AUTONOMY_PLAIN[level].alone)).toBeInTheDocument();
    expect(screen.getByText(AUTONOMY_PLAIN[level].asks)).toBeInTheDocument();
  });

  it('falls back to the description for a custom agent', async () => {
    card(agent({ id: 'c1', name: 'Zed', category: 'custom', description: 'Sorts supplier quotes.' }));
    await userEvent.setup().click(screen.getByRole('button', { name: 'How I work' }));
    const article = screen.getByRole('article', { name: 'Zed' });
    expect(within(article).getByText('Sorts supplier quotes.')).toBeInTheDocument();
    expect(within(article).getByText('Custom helper')).toBeInTheDocument();
    expect(within(article).getByText(/matches the rules this helper was set up with/)).toBeInTheDocument();
  });

  it('never shows a placeholder for the personal agent', () => {
    expect(agentDisplayName({ name: 'Meera', scope: 'personal' })).toBe('Your Meera');
    expect(agentDisplayName({ name: 'My assistant', scope: 'personal' })).toBe('Your Meera');
    expect(agentDisplayName({ name: 'Meera (personal)', scope: 'personal' })).toBe('Your Meera');
    expect(agentDisplayName({ name: 'Kabir', scope: 'workspace' })).toBe('Kabir');
  });
});

describe('hub intro and avatar changes', () => {
  beforeEach(() => localStorage.setItem(TOKEN_KEY, 'tok'));
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  function serve() {
    const lead = agent({ id: 'a-lead', name: 'Routing', category: 'general', kind: 'lead', template_key: 'general.lead', hidden: true });
    const kabir = agent({ id: 'a-kabir', name: 'Kabir', category: 'sales', template_key: 'sales.qualifier', avatar: '🎯' });
    const mine = agent({ id: 'a-me', name: 'Meera', category: 'personal', scope: 'personal', template_key: 'personal.mailbox', owner_user_id: 'u1', avatar: '🙋' });
    const server = { mine };
    const patches: string[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string, init?: RequestInit) => {
        if (url.endsWith('/me')) return Promise.resolve(json(meBody('admin')));
        if (init?.method === 'PATCH') {
          const body = JSON.parse(init.body as string) as { avatar?: string };
          patches.push(init.body as string);
          server.mine = { ...server.mine, ...(body.avatar ? { avatar: body.avatar } : {}) };
          return Promise.resolve(json(server.mine));
        }
        if (url.endsWith('/outgoing') || url.endsWith('/versions') || url.endsWith('/audit')) return Promise.resolve(json({ items: [] }));
        return Promise.resolve(json(registry([lead, kabir], server.mine)));
      }),
    );
    return patches;
  }

  const mount = () =>
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ToastProvider>
          <MemoryRouter initialEntries={['/w/w1/agent-team']}>
            <Routes>
              <Route path="/w/:ws/agent-team" element={<AgentsHubPage />} />
            </Routes>
          </MemoryRouter>
        </ToastProvider>
      </QueryClientProvider>,
    );

  const faceOf = (name: string) => within(screen.getByRole('article', { name })).getByTestId('agent-face').querySelector('img');

  it('opens with a short intro card for the team', async () => {
    serve();
    mount();
    const intro = await screen.findByRole('complementary', { name: 'About your team' });
    expect(intro).toHaveTextContent('Meera is your own assistant');
    expect(intro).toHaveTextContent('one expert');
    expect(intro).toHaveTextContent('ask only for sensitive things');
    expect(intro).not.toHaveTextContent('Aarav');
  });

  it('shows name and role together, and the personal agent as "Your Meera"', async () => {
    serve();
    mount();
    const kabir = await screen.findByRole('article', { name: 'Kabir' });
    expect(within(kabir).getByText('Sales expert')).toBeInTheDocument();
    expect(screen.getByRole('article', { name: 'Your Meera' })).toBeInTheDocument();
    expect(screen.queryByText(/My assistant/)).not.toBeInTheDocument();
    expect(screen.queryByRole('article', { name: /Routing|Aarav/ })).not.toBeInTheDocument();
  });

  it('shows a new avatar straight away and keeps it after a fresh load', async () => {
    const patches = serve();
    const user = userEvent.setup();
    const first = mount();
    await screen.findByRole('article', { name: 'Your Meera' });
    expect(faceOf('Your Meera')).toHaveAttribute('src', '/avatars/blossom.webp');
    await user.click(screen.getByRole('button', { name: 'Open Your Meera' }));
    await user.click(await screen.findByRole('radio', { name: 'Owl' }));
    await waitFor(() => expect(faceOf('Your Meera')).toHaveAttribute('src', '/avatars/owl.webp'));
    expect(patches).toEqual([JSON.stringify({ avatar: 'fluent:owl' })]);
    expect(within(screen.getByRole('dialog')).getAllByTestId('agent-face')[0]?.querySelector('img')).toHaveAttribute('src', '/avatars/owl.webp');
    first.unmount();

    mount();
    await screen.findByRole('article', { name: 'Your Meera' });
    expect(faceOf('Your Meera')).toHaveAttribute('src', '/avatars/owl.webp');
  });
});
