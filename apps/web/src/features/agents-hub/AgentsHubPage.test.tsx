import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TOKEN_KEY } from '../../api/client';
import { ToastProvider } from '../../components/ui';
import { agent, json, meBody, registry } from '../../test/fixtures';
import { AgentsHubPage, groupAgents } from './AgentsHubPage';

const lead = agent({ id: 'a-lead', name: 'Routing', category: 'general', kind: 'lead', hidden: true, template_key: 'general.lead', autonomy: { earned: 1, cap: 3, effective: 1, limited_by: 'earned' } });
const desk = agent({ id: 'a-pay', name: 'Aditya', category: 'hrms', template_key: 'hrms.payroll', autonomy: { earned: 2, cap: 3, effective: 2, limited_by: 'earned' }, today: { routed: 3, cases: 3, approvals_pending: 2, cost_usd: 0 } });
const leave = agent({ id: 'a-leave', name: 'Ananya', category: 'hrms', template_key: 'hrms.leave' });
const support = agent({ id: 'a-sup', name: 'Ishaan', category: 'support', template_key: 'support.resolver', status: 'paused', paused_reason: 'pack_off' });
const mine = agent({ id: 'a-me', name: 'Meera', category: 'personal', scope: 'personal', template_key: 'personal.mailbox', owner_user_id: 'u1', avatar: 'fluent:koala' });

function setup(role: 'admin' | 'operator' = 'admin') {
  const calls: { url: string; method: string; body?: string }[] = [];
  vi.stubGlobal(
    'fetch',
    vi.fn((url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET';
      calls.push({ url, method, body: init?.body as string | undefined });
      if (url.endsWith('/me')) return Promise.resolve(json(meBody(role)));
      if (url.endsWith('/outgoing')) return Promise.resolve(json({ items: [{ draft_id: 'd1', message_id: 'm1', subject: 'Re: Salary slip', to: 'ravi@x.test', send_at: new Date(Date.now() + 600_000).toISOString(), body_preview: 'Hi Ravi' }] }));
      if (url.endsWith('/versions')) return Promise.resolve(json({ items: [{ version: 2, source: 'tuned', gate_status: 'passed', note: null, spec_hash: 'x', active: true, created_at: new Date().toISOString() }, { version: 1, source: 'template', gate_status: 'passed', note: null, spec_hash: 'y', active: false, created_at: new Date().toISOString() }] }));
      if (method !== 'GET') return Promise.resolve(json({ status: 'cancelled', draft_id: 'd1' }));
      return Promise.resolve(json(registry([lead, desk, leave, support], mine)));
    }),
  );
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
  return calls;
}

describe('groupAgents', () => {
  it('orders categories, lifts the personal agent out, hides the routing lead and drops empty groups', () => {
    const groups = groupAgents(registry([desk, lead, leave, support], mine).categories, mine.id);
    expect(groups.map((g) => g.title)).toEqual(['Support', 'HRMS']);
    expect(groups[1]?.agents.map((a) => a.name)).toEqual(['Aditya', 'Ananya']);
  });
});

describe('AgentsHubPage', () => {
  beforeEach(() => localStorage.setItem(TOKEN_KEY, 'tok'));
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('groups agents by category and says what each can do in plain words', async () => {
    setup();
    expect(await screen.findByRole('heading', { name: 'Your personal agent' })).toBeInTheDocument();
    const headings = screen.getAllByRole('heading', { level: 2 }).map((h) => h.textContent);
    expect(headings).toEqual(['Your personal agent', 'Support1', 'HRMS2']);
    const payroll = screen.getByRole('article', { name: 'Aditya' });
    expect(within(payroll).getByText('Sends routine replies after a 10-minute hold')).toBeInTheDocument();
    expect(within(payroll).getByText(/2 waiting on you/)).toBeInTheDocument();
    expect(screen.queryByRole('article', { name: /Routing|Aarav/ })).not.toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/gpt|claude|gemini|anthropic|openai|haiku|sonnet|opus/i);
  });

  it('turns an agent off with a PATCH and keeps pack-off agents locked', async () => {
    const calls = setup();
    const user = userEvent.setup();
    await user.click(await screen.findByRole('switch', { name: 'Pause Aditya' }));
    await waitFor(() =>
      expect(calls).toContainEqual(expect.objectContaining({ url: '/api/v1/workspaces/w1/agents/a-pay', method: 'PATCH', body: JSON.stringify({ status: 'paused' }) })),
    );
    expect(screen.getByRole('switch', { name: 'Turn on Ishaan' })).toBeDisabled();
    expect(screen.getByText('Its pack is turned off')).toBeInTheDocument();
  });

  it('lets members use their own agent switch but not the workspace desks', async () => {
    setup('operator');
    expect(await screen.findByRole('switch', { name: 'Pause Aditya' })).toBeDisabled();
    expect(screen.getByRole('switch', { name: 'Pause Your Meera' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Pause all' })).not.toBeInTheDocument();
  });

  it('saves a picked avatar for the personal agent', async () => {
    const calls = setup();
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: 'Open Your Meera' }));
    await user.click(await screen.findByRole('radio', { name: 'Owl' }));
    await waitFor(() => expect(calls).toContainEqual(expect.objectContaining({ method: 'PATCH', body: JSON.stringify({ avatar: 'fluent:owl' }) })));
  });

  it('shows held sends with Stop and cancels one', async () => {
    const calls = setup();
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: 'Open Aditya' }));
    await user.click(await screen.findByRole('tab', { name: 'Going out' }));
    expect(await screen.findByText('Re: Salary slip')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Stop sending Re: Salary slip' }));
    await waitFor(() => expect(calls).toContainEqual(expect.objectContaining({ url: '/api/v1/workspaces/w1/drafts/d1/undo-send', method: 'POST' })));
  });

  it('rolls back to an earlier version', async () => {
    const calls = setup();
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: 'Open Aditya' }));
    await user.click(screen.getByRole('tab', { name: 'Versions' }));
    await user.click(await screen.findByRole('button', { name: 'Roll back to version 1' }));
    await waitFor(() => expect(calls).toContainEqual(expect.objectContaining({ url: '/api/v1/workspaces/w1/agents/a-pay/versions/1/activate', method: 'POST' })));
  });

  it('shows Create an agent to admins only', async () => {
    setup();
    await screen.findByRole('heading', { name: 'Your personal agent' });
    expect(await screen.findByRole('link', { name: 'Create an agent' })).toHaveAttribute('href', '/w/w1/agents/new');
  });

  it('hides Create an agent from operators', async () => {
    setup('operator');
    await screen.findByRole('heading', { name: 'Your personal agent' });
    expect(screen.queryByRole('link', { name: 'Create an agent' })).not.toBeInTheDocument();
  });
});
