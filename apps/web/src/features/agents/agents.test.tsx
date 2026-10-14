import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ToastProvider } from '../../components/ui';
import { MatchingPage } from '../matching/MatchingPage';
import { AgentsBadge } from './AgentsBadge';
import { AgentsPage } from './AgentsPage';
import { CaseDetailPage } from './CaseDetailPage';
import { formatCents } from './labels';
import { StartAgentButton } from './StartAgentButton';
import type { Approval, CaseDetail, CaseSummary } from './types';

const approval = (over: Partial<Approval> = {}): Approval => ({
  id: 'ap1',
  case_id: 'c1',
  case_title: 'Senior React Developer at Capgemini',
  action: 'submit_candidate',
  summary: 'Send submission of 2 candidates to hr@capgemini.com',
  to: 'hr@capgemini.com',
  subject: 'Profiles for Senior React Developer',
  body: 'Hi team,\n\nPlease find two profiles.',
  candidates: [
    { id: 'k1', name: 'Asha Rao', score: 93 },
    { id: 'k2', name: 'Ravi Kumar', score: 81 },
  ],
  status: 'pending',
  decided_by: null,
  decided_at: null,
  created_at: new Date(Date.now() - 3_600_000).toISOString(),
  ...over,
});

const summary = (over: Partial<CaseSummary> = {}): CaseSummary => ({
  id: 'c1',
  kind: 'requirement',
  title: 'Senior React Developer at Capgemini',
  goal: 'Shortlist up to 3 candidates and submit',
  status: 'waiting_event',
  outcome: 'submitted',
  steps: 6,
  max_steps: 15,
  spent_usd: 0.021,
  budget_usd: 0.1,
  message_id: 'm1',
  requirement_id: 'r1',
  next_wake_at: '2026-10-15T10:00:00',
  created_at: '2026-10-14T09:00:00Z',
  updated_at: '2026-10-14T09:30:00Z',
  ...over,
});

const detail = (): CaseDetail => {
  const { steps: _steps, ...rest } = summary({ status: 'waiting_approval', outcome: null });
  return {
    ...rest,
    state: {},
    steps: [
      { seq: 1, kind: 'tool', tool: 'find_candidates', summary: 'Found 3 candidates (best 93)', input: { requirement_id: 'r1' }, output: { n: 3 }, status: 'ok', cost_usd: 0, created_at: '2026-10-14T09:01:00Z' },
      { seq: 2, kind: 'critic', tool: null, summary: 'Checked the draft against the requirement', input: {}, output: {}, status: 'ok', cost_usd: 0.0042, created_at: '2026-10-14T09:02:00Z' },
      { seq: 3, kind: 'approval', tool: 'submit_candidate', summary: 'Waiting for your OK to submit 2 candidates', input: {}, output: {}, status: 'waiting', cost_usd: 0.0123, created_at: '2026-10-14T09:03:00Z' },
    ],
    approvals: [approval()],
  };
};

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });
const ME = (role: 'admin' | 'operator') => ({
  user: { id: 'u', email: 'a@b.test', name: null },
  workspaces: [{ id: 'w1', name: 'W', role, created_at: '' }],
  auth_mode: 'password',
});

type Handler = (url: string, init?: RequestInit) => Response | undefined;

function mockFetch(handler: Handler) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => Promise.resolve(handler(url, init) ?? json({ items: [] })));
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

function renderAt(path: string, routes: { path: string; element: React.ReactNode }[], toasts = true) {
  const tree = (
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        {routes.map((r) => (
          <Route key={r.path} path={r.path} element={r.element} />
        ))}
      </Routes>
    </MemoryRouter>
  );
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      {toasts ? <ToastProvider>{tree}</ToastProvider> : tree}
    </QueryClientProvider>,
  );
}

const renderAgents = () => renderAt('/w/w1/agents', [{ path: '/w/:ws/agents', element: <AgentsPage /> }]);

const postsTo = (fetchMock: ReturnType<typeof mockFetch>, suffix: string) =>
  fetchMock.mock.calls
    .filter(([url, init]) => init?.method === 'POST' && String(url).endsWith(suffix))
    .map(([, init]) => (init?.body ? JSON.parse(init.body as string) : {}) as Record<string, unknown>);

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe('formatCents', () => {
  it('shows cost in cents', () => {
    expect(formatCents(0.021)).toBe('2.1¢');
    expect(formatCents(0.1)).toBe('10¢');
    expect(formatCents(0)).toBe('0¢');
    expect(formatCents(0.00001)).toBe('<0.1¢');
  });
});

describe('AgentsBadge', () => {
  it('shows pending_count from GET /approvals?status=pending', async () => {
    const fetchMock = mockFetch(() => json({ items: [approval()], pending_count: 4 }));
    renderAt('/w/w1/agents', [{ path: '/w/:ws/agents', element: <AgentsBadge ws="w1" /> }]);

    expect(await screen.findByText('4 approvals waiting')).toBeInTheDocument();
    expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/v1/workspaces/w1/approvals?status=pending');
  });

  it('renders nothing when nothing is pending', async () => {
    const fetchMock = mockFetch(() => json({ items: [], pending_count: 0 }));
    const { container } = renderAt('/w/w1/agents', [{ path: '/w/:ws/agents', element: <AgentsBadge ws="w1" /> }], false);

    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(container).toBeEmptyDOMElement();
  });
});

describe('AgentsPage approvals', () => {
  const pending = (handler?: Handler) =>
    mockFetch((url, init) => {
      const custom = handler?.(url, init);
      if (custom) return custom;
      if (url.includes('/approvals/')) return json(approval({ status: 'approved' }));
      if (url.includes('/approvals')) return json({ items: [approval()], pending_count: 1 });
      return undefined;
    });

  it('shows the approval with recipient, subject, and candidate scores', async () => {
    pending();
    renderAgents();

    expect(await screen.findByText('Send submission of 2 candidates to hr@capgemini.com')).toBeInTheDocument();
    expect(screen.getByText('Profiles for Senior React Developer')).toBeInTheDocument();
    const chips = screen.getByRole('list', { name: 'Candidates in this submission' });
    expect(within(chips).getByText('Asha Rao')).toBeInTheDocument();
    expect(within(chips).getByLabelText('score 93 of 100')).toBeInTheDocument();
    expect(screen.getByLabelText('Email body')).toHaveValue('Hi team,\n\nPlease find two profiles.');
  });

  it('approves with edited_body only when the body was edited', async () => {
    const fetchMock = pending();
    renderAgents();

    const body = await screen.findByLabelText('Email body');
    expect(body).toHaveAttribute('readonly');
    await userEvent.click(screen.getByRole('button', { name: 'Edit' }));
    await userEvent.clear(body);
    await userEvent.type(body, 'Hello, two strong profiles attached.');
    await userEvent.click(screen.getByRole('button', { name: 'Send' }));

    await waitFor(() => expect(postsTo(fetchMock, '/approvals/ap1')).toHaveLength(1));
    expect(postsTo(fetchMock, '/approvals/ap1')[0]).toEqual({ decision: 'approve', edited_body: 'Hello, two strong profiles attached.' });
    expect(await screen.findByText('Approved and sent to hr@capgemini.com.')).toBeInTheDocument();
  });

  it('approves without edited_body when untouched', async () => {
    const fetchMock = pending();
    renderAgents();

    await userEvent.click(await screen.findByRole('button', { name: 'Send' }));

    await waitFor(() => expect(postsTo(fetchMock, '/approvals/ap1')).toHaveLength(1));
    expect(postsTo(fetchMock, '/approvals/ap1')[0]).toEqual({ decision: 'approve' });
  });

  it('declines with an optional note', async () => {
    const fetchMock = pending();
    renderAgents();

    await userEvent.click(await screen.findByRole('button', { name: 'Not now' }));
    await userEvent.type(screen.getByLabelText('Note for the agent (optional)'), 'Wrong client contact');
    await userEvent.click(screen.getByRole('button', { name: 'Confirm not now' }));

    await waitFor(() => expect(postsTo(fetchMock, '/approvals/ap1')).toHaveLength(1));
    expect(postsTo(fetchMock, '/approvals/ap1')[0]).toEqual({ decision: 'decline', note: 'Wrong client contact' });
    expect(await screen.findByText('Declined. Nothing was sent.')).toBeInTheDocument();
  });

  it('shows a 409 from the server inline and keeps the card', async () => {
    pending((url, init) =>
      init?.method === 'POST' && url.includes('/approvals/')
        ? json({ error: { code: 'conflict', message: 'Draft still has [NEEDS: salary] gaps' } }, 409)
        : undefined,
    );
    renderAgents();

    await userEvent.click(await screen.findByRole('button', { name: 'Send' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('Draft still has [NEEDS: salary] gaps');
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    expect(screen.getByText('Could not send. Nothing was sent.')).toBeInTheDocument();
  });
});

describe('AgentsPage cases', () => {
  const cases = (items: CaseSummary[]) =>
    mockFetch((url) => {
      if (url.includes('/approvals')) return json({ items: [], pending_count: 0 });
      if (url.includes('/cases')) return json({ items });
      return undefined;
    });

  it('lists cases with plain-language status, outcome, steps, cost, and next check', async () => {
    cases([summary()]);
    renderAgents();

    expect(await screen.findByRole('link', { name: 'Senior React Developer at Capgemini' })).toHaveAttribute('href', '/w/w1/agents/c1');
    expect(screen.getByText('Waiting for reply')).toBeInTheDocument();
    expect(screen.getByText('Submitted')).toBeInTheDocument();
    expect(screen.getByText('6 of 15 steps')).toBeInTheDocument();
    expect(screen.getByText('2.1¢')).toBeInTheDocument();
    expect(screen.getByText(/checks again \w{3} 10:00/)).toBeInTheDocument();
    expect(screen.getByText('Requirement:')).toBeInTheDocument();
  });

  it('sends the filter as the status query param', async () => {
    const fetchMock = cases([summary()]);
    renderAgents();
    await screen.findByText('Waiting for reply');

    await userEvent.click(screen.getByRole('tab', { name: 'Working' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/cases?status=running'))).toBe(true));

    await userEvent.click(screen.getByRole('tab', { name: 'Done' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/cases?status=done'))).toBe(true));

    await userEvent.click(screen.getByRole('tab', { name: 'Failed' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/cases?status=failed'))).toBe(true));

    expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/cases'))).toBe(true);
  });

  it('Waiting filter keeps only waiting cases', async () => {
    cases([summary({ id: 'c1', title: 'Waiting one' }), summary({ id: 'c2', title: 'Busy one', status: 'running', outcome: null })]);
    renderAgents();
    await screen.findByText('Busy one');

    await userEvent.click(screen.getByRole('tab', { name: 'Waiting' }));

    expect(screen.getByText('Waiting one')).toBeInTheDocument();
    expect(screen.queryByText('Busy one')).not.toBeInTheDocument();
  });
});

describe('CaseDetailPage', () => {
  const open = (role: 'admin' | 'operator', fetchMock?: Handler) => {
    localStorage.setItem('auth_token', 'dev:a@b.test');
    const mock = mockFetch((url, init) => {
      const custom = fetchMock?.(url, init);
      if (custom) return custom;
      if (url.endsWith('/me')) return json(ME(role));
      if (url.endsWith('/cases/c1/cancel')) return json(summary({ status: 'cancelled' }));
      if (url.endsWith('/cases/c1')) return json(detail());
      if (url.includes('/approvals')) return json({ items: [], pending_count: 0 });
      return undefined;
    });
    renderAt('/w/w1/agents/c1', [{ path: '/w/:ws/agents/:caseId', element: <CaseDetailPage /> }]);
    return mock;
  };

  it('renders the timeline with summaries, costs, budget, and the approval inline', async () => {
    open('operator');

    expect(await screen.findByText('Found 3 candidates (best 93)')).toBeInTheDocument();
    expect(screen.getByText('Checked the draft against the requirement')).toBeInTheDocument();
    expect(screen.getByText('Waiting for your OK to submit 2 candidates')).toBeInTheDocument();
    expect(screen.getByText('0¢')).toBeInTheDocument();
    expect(screen.getByText('0.4¢')).toBeInTheDocument();
    expect(screen.getByText('1.2¢')).toBeInTheDocument();
    expect(screen.getByRole('progressbar', { name: 'Budget used' })).toHaveAttribute('aria-valuetext', '2.1¢ of 10¢ budget');
    expect(screen.getByRole('button', { name: 'Send' })).toBeInTheDocument();
    expect(screen.queryByText('Technical details')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Cancel case' })).not.toBeInTheDocument();
  });

  it('shows raw JSON and cancel (with confirmation) to admins', async () => {
    const fetchMock = open('admin');

    const details = await screen.findAllByText('Technical details');
    expect(details).toHaveLength(3);
    expect(screen.getAllByText(/"requirement_id": "r1"/)[0]).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Cancel case' }));
    expect(postsTo(fetchMock, '/cancel')).toHaveLength(0);
    await userEvent.click(screen.getByRole('button', { name: 'Yes, cancel case' }));

    await waitFor(() => expect(postsTo(fetchMock, '/cases/c1/cancel')).toHaveLength(1));
  });
});

describe('start agent entry points', () => {
  it('inbox button posts kind inbox with the message id and links to the case', async () => {
    const fetchMock = mockFetch(() => json(summary({ id: 'c9', kind: 'inbox' })));
    renderAt('/w/w1/inbox/m7', [
      { path: '/w/:ws/inbox/:id', element: <StartAgentButton ws="w1" target={{ kind: 'inbox', message_id: 'm7' }} label="Let the agent handle it" /> },
    ]);

    await userEvent.click(screen.getByRole('button', { name: 'Let the agent handle it' }));

    await waitFor(() => expect(postsTo(fetchMock, '/cases')).toEqual([{ kind: 'inbox', message_id: 'm7' }]));
    expect(await screen.findByRole('link', { name: 'View case' })).toHaveAttribute('href', '/w/w1/agents/c9');
  });

  it('Matching requirement card posts kind requirement with the requirement id', async () => {
    const requirement = {
      id: 'r1', message_id: 'm1', client: 'Acme', role_title: 'Senior React Developer', must_have_skills: ['React'],
      nice_to_have_skills: [], min_experience: 5, max_experience: 8, location: 'Hyderabad', positions: 1, status: 'open',
      source: 'llm', created_at: '', match_count: 0, top_score: null,
    };
    const fetchMock = mockFetch((url, init) => {
      if (init?.method === 'POST') return json(summary({ id: 'c5' }));
      if (url.includes('/matching/requirements')) return json({ items: [requirement] });
      return undefined;
    });
    renderAt('/w/w1/matching', [{ path: '/w/:ws/matching', element: <MatchingPage /> }]);

    await userEvent.click(await screen.findByRole('button', { name: 'Start agent for Senior React Developer' }));

    await waitFor(() => expect(postsTo(fetchMock, '/cases')).toEqual([{ kind: 'requirement', requirement_id: 'r1' }]));
    expect(await screen.findByRole('link', { name: 'View case' })).toHaveAttribute('href', '/w/w1/agents/c5');
  });

  it('shows the server error when starting fails', async () => {
    mockFetch(() => json({ error: { code: 'conflict', message: 'A case is already running for this email' } }, 409));
    renderAt('/w/w1/inbox/m7', [
      { path: '/w/:ws/inbox/:id', element: <StartAgentButton ws="w1" target={{ kind: 'inbox', message_id: 'm7' }} label="Let the agent handle it" /> },
    ]);

    await userEvent.click(screen.getByRole('button', { name: 'Let the agent handle it' }));

    expect(await screen.findByRole('alert')).toHaveTextContent('A case is already running for this email');
  });
});
