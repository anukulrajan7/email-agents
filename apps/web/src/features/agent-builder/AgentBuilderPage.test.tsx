import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AgentBuilderPage } from './AgentBuilderPage';
import { describeSending, describeStep, describeTrigger } from './describe';
import type { DesignerSession } from './types';

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const ME = (role: 'admin' | 'operator') => ({
  user: { id: 'u', email: 'a@b.test', name: null },
  workspaces: [{ id: 'w1', name: 'W', role, created_at: '' }],
  auth_mode: 'password',
});

const session = (over: Partial<DesignerSession> = {}): DesignerSession => ({
  id: 's1',
  goal: 'refunds',
  agent_id: null,
  base_version: null,
  status: 'ready',
  turns: [
    { role: 'user', text: 'refunds' },
    { role: 'designer', text: 'Here is "Refund Helper".' },
  ],
  spec: {
    name: 'Refund Helper',
    key: 'refund_helper',
    description: 'Collects the order number and drafts a reply.',
    goal: 'g',
    mode: 'workflow',
    triggers: [{ type: 'email_received', match: { labels: ['billing'], from_domains: [], from_addresses: [], subject_keywords: ['refund'], needs_reply: null, priority: null } }],
    steps: [
      { type: 'extract', fields: [{ name: 'order_id', type: 'string', description: '' }] },
      { type: 'draft_reply', quality: 'balanced' },
    ],
    permissions: { draft: true, send: 'ask', max_autonomy: 1 },
    drafting: { rules: ['Be brief'], never_do: [] },
  },
  issues: [],
  questions: [],
  assumptions: [],
  dry_run: null,
  cost_usd: 0.01,
  limits: { calls_today: 1, daily_limit: 20, session_budget_usd: 0.1 },
  ...over,
});

const DONE = {
  status: 'done' as const,
  would_handle: 1,
  sampled: 2,
  steals: [],
  examples: [{ message_id: 'm1', subject: 'Refund for my order', route_reason: 'Taken because it matches the label', currently_handled_by: null, steps: [{ summary: 'Pulled out order_id: 4411', status: 'ok' }], draft: 'Hi Ravi', needs_person: true }],
  est_cost_per_email_usd: 0.004,
  est_monthly_usd: 0.12,
  warnings: [],
};

function setup(role: 'admin' | 'operator', handler: (url: string, init?: RequestInit) => Response | undefined, path = '/w/w1/agents/new') {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => Promise.resolve(handler(url, init) ?? (url.endsWith('/me') ? json(ME(role)) : json({ items: [] }))));
  localStorage.setItem('auth_token', 't');
  vi.stubGlobal('fetch', fetchMock);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/w/:ws/agents/new" element={<AgentBuilderPage />} />
          <Route path="/w/:ws/agents" element={<p>Agents list</p>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return fetchMock;
}

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe('plain words', () => {
  it('describes triggers, steps and sending', () => {
    const s = session().spec!;
    expect(describeTrigger(s.triggers[0]!)).toBe('When an email arrives labelled billing, with “refund” in the subject');
    expect(s.steps.map(describeStep)).toEqual(['Pull out order id', 'Write a reply draft']);
    expect(describeSending(s.permissions)).toBe('It asks you before anything is sent.');
  });
});

describe('AgentBuilderPage', () => {
  it('turns a description into a plan, tries it and saves it', async () => {
    let current = session();
    const fetchMock = setup('admin', (url, init) => {
      if (url.endsWith('/designer/sessions') && init?.method === 'POST') return json(current, 201);
      if (url.endsWith('/dry-run')) {
        current = session({ dry_run: DONE });
        return json(current, 202);
      }
      if (url.endsWith('/save')) return json({ agent: { id: 'a1', name: 'Refund Helper' }, version: 1, session: session({ status: 'saved', dry_run: DONE }) });
      if (url.endsWith('/designer/sessions/s1')) return json(current);
      return undefined;
    });
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText('What should this agent do?'), 'refunds');
    await user.click(screen.getByRole('button', { name: 'Design the agent' }));

    expect(await screen.findByRole('heading', { name: 'Refund Helper' })).toBeInTheDocument();
    expect(screen.getByText('Pull out order id')).toBeInTheDocument();
    expect(screen.getByText('It asks you before anything is sent.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save the agent' })).toBeDisabled();

    await user.click(screen.getByRole('button', { name: 'Try it on recent mail' }));
    expect(await screen.findByText(/Estimated cost/)).toBeInTheDocument();
    expect(screen.getByText('Refund for my order')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Save the agent' }));
    expect(await screen.findByText('Agents list')).toBeInTheDocument();
    const save = fetchMock.mock.calls.find(([u]) => String(u).endsWith('/save'));
    expect(JSON.parse(save?.[1]?.body as string)).toEqual({ start_level: 0, confirm_steals: false });
  });

  it('asks you to agree before an agent takes mail from another agent', async () => {
    const steals = { ...DONE, steals: [{ agent_id: 'a2', name: 'Inbox Assistant', count: 1 }] };
    const fetchMock = setup('admin', (url) => {
      if (url.endsWith('/save')) return json({ agent: { id: 'a1', name: 'Refund Helper' }, version: 1, session: session({ status: 'saved', dry_run: steals }) });
      if (url.endsWith('/designer/sessions/s1')) return json(session({ dry_run: steals }));
      return undefined;
    }, '/w/w1/agents/new?session=s1');
    const user = userEvent.setup();
    const save = await screen.findByRole('button', { name: 'Save the agent' });
    expect(save).toBeDisabled();
    await user.click(screen.getByRole('checkbox', { name: /Inbox Assistant/ }));
    await user.click(save);
    await screen.findByText('Agents list');
    const call = fetchMock.mock.calls.find(([u]) => String(u).endsWith('/save'));
    expect(JSON.parse(call?.[1]?.body as string)).toEqual({ start_level: 0, confirm_steals: true });
  });

  it('shows problems in words and blocks the try-out', async () => {
    setup(
      'admin',
      (url) => {
        if (url.endsWith('/designer/sessions/s1')) return json(session({ status: 'drafting', issues: [{ code: 'unknown_label', message: 'The label nope does not exist.' }] }));
        return undefined;
      },
      '/w/w1/agents/new?session=s1',
    );
    expect(await screen.findByText('The label nope does not exist.')).toBeInTheDocument();
    await waitFor(() => expect(screen.getByRole('button', { name: 'Try it on recent mail' })).toBeDisabled());
  });

  it('shows a friendly failure with a Retry that asks again', async () => {
    const user = userEvent.setup();
    let asked = 0;
    setup(
      'admin',
      (url) => {
        if (url.endsWith('/messages')) {
          asked += 1;
          return json(session());
        }
        if (url.endsWith('/designer/sessions/s1')) return json(session({ status: 'failed', spec: null, issues: [{ code: 'could_not_build', message: 'x' }], assumptions: ['Step 1 had no type.'] }));
        return undefined;
      },
      '/w/w1/agents/new?session=s1',
    );
    expect(await screen.findByText(/couldn.t build that agent/)).toBeInTheDocument();
    expect(screen.getByText('Step 1 had no type.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Retry' }));
    await waitFor(() => expect(asked).toBe(1));
  });

  it('is for admins only', async () => {
    setup('operator', () => undefined);
    expect(await screen.findByText('Only admins can create agents')).toBeInTheDocument();
  });
});
