import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ToastProvider } from '../../components/ui';
import { AgentsPage } from './AgentsPage';
import { OnboardingPrompt } from './OnboardingPrompt';
import type { AgentView, Approval, MyProfileView, TeamProfile, UserProfile } from './types';

const agentView = (canEdit: boolean): AgentView => ({
  agent: {
    name: 'Meera',
    avatar: '🌼',
    color: 'ochre',
    persona: "I'm Meera, the team's email assistant.",
    signature: 'Acme HR',
    tasks: ['reply', 'requirements'],
    permissions: { draft: true, send: 'ask', submit_candidate: 'ask' },
    quality: 'balanced',
    enabled: true,
    updated_at: '2026-10-14T09:00:00Z',
  },
  standard_guidance: 'Standard guidance for an HR team.',
  can_edit: canEdit,
});

const blankProfile: UserProfile = {
  display_name: '',
  job_title: '',
  handles: [],
  handles_note: '',
  ignore_note: '',
  audiences: [],
  vips: [],
  tone: { formality: '', length: '', sign_off: null, language: '' },
  working_hours: '',
  timezone: '',
  never_do: '',
  approvals: { send_email: 'always_ask', submit_candidate: 'always_ask' },
  saved: false,
  updated_at: null,
};

const myProfile = (over: Partial<MyProfileView> = {}): MyProfileView => ({
  profile: blankProfile,
  agent: { name: 'Meera', avatar: '🌼', color: 'ochre' },
  mailboxes: ['priya@acme.com'],
  needs_onboarding: false,
  ...over,
});

const approval = (over: Partial<Approval> = {}): Approval => ({
  id: 'ap1',
  case_id: 'c1',
  case_title: 'Status update',
  action: 'send_email',
  summary: 'Send reply to ravi@client.com',
  to: 'ravi@client.com',
  subject: 'Re: Status update',
  body: 'Hello Ravi,\n\nThanks.',
  candidates: [],
  status: 'pending',
  decided_by: null,
  decided_at: null,
  created_at: new Date().toISOString(),
  agent: { name: 'Meera', avatar: '🌼', color: 'ochre' },
  working_for: { user_id: 'u1', email: 'priya@acme.com', name: 'Priya' },
  ...over,
});

const team: TeamProfile[] = [
  { user_id: 'u1', email: 'priya@acme.com', role: 'operator', mailboxes: ['priya@acme.com'], profile: { ...blankProfile, display_name: 'Priya', job_title: 'Recruiter', saved: true } },
  { user_id: 'u2', email: 'admin@acme.com', role: 'admin', mailboxes: [], profile: null },
];

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status });

function mockApi(role: 'admin' | 'operator', opts: { approvals?: Approval[]; mine?: MyProfileView } = {}) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    const method = init?.method ?? 'GET';
    const path = url.split('?')[0] ?? url;
    if (path === '/api/v1/me')
      return Promise.resolve(json({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role, created_at: '' }], auth_mode: 'password' }));
    if (path.endsWith('/agent') && method === 'GET') return Promise.resolve(json(agentView(role === 'admin')));
    if (path.endsWith('/agent') && method === 'PUT') return Promise.resolve(json({ ...agentView(true).agent, ...JSON.parse(String(init?.body)) }));
    if (path.endsWith('/agent/profiles')) return Promise.resolve(json({ items: team }));
    if (path.endsWith('/agent/preview')) return Promise.resolve(json({ body: 'Hello Ravi,\n\nNoted, thanks!\n\nMeera', needs_reply: true, agent: agentView(true).agent }));
    if (path.endsWith('/me/agent-profile/skip')) return Promise.resolve(json({ skipped: true }));
    if (path.endsWith('/me/agent-profile') && method === 'PUT') return Promise.resolve(json({ ...blankProfile, saved: true }));
    if (path.endsWith('/me/agent-profile')) return Promise.resolve(json(opts.mine ?? myProfile()));
    if (path.endsWith('/messages'))
      return Promise.resolve(json({ items: [{ id: 'm1', subject: 'Status update', from: 'ravi@client.com' }], next_cursor: null }));
    if (path.endsWith('/approvals')) return Promise.resolve(json({ items: opts.approvals ?? [], pending_count: opts.approvals?.length ?? 0 }));
    return Promise.resolve(json({ items: [] }));
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  return fetchMock;
}

function renderAgents(element: React.ReactNode = <AgentsPage />) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ToastProvider>
        <MemoryRouter initialEntries={['/w/w1/agents']}>
          <Routes>
            <Route path="/w/:ws/agents" element={element} />
          </Routes>
        </MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>,
  );
}

const bodiesOf = (fetchMock: ReturnType<typeof mockApi>, method: string, suffix: string) =>
  fetchMock.mock.calls
    .filter(([url, init]) => init?.method === method && String(url).split('?')[0]?.endsWith(suffix))
    .map(([, init]) => (init?.body ? JSON.parse(init.body as string) : {}) as Record<string, unknown>);

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe('Your agent', () => {
  it('describes the agent in plain words, without model names', async () => {
    mockApi('operator');
    renderAgents();
    const card = await screen.findByRole('region', { name: 'Your agent' });
    expect(await within(card).findByText('Meera')).toBeInTheDocument();
    expect(within(card).getByText('Reply to emails · Work on client requirements')).toBeInTheDocument();
    expect(within(card).getByText('Asks before sending')).toBeInTheDocument();
    expect(within(card).getByText('Balanced')).toBeInTheDocument();
    expect(within(card).getByText('Only a workspace admin can change the agent.')).toBeInTheDocument();
    expect(card.textContent).not.toMatch(/gpt|claude|kimi|deepseek|qwen|haiku|openrouter|model/i);
  });

  it('lets an admin edit name, avatar, persona, permissions and quality', async () => {
    const fetchMock = mockApi('admin');
    renderAgents();
    await userEvent.click(await screen.findByRole('button', { name: 'Edit Meera' }));
    const drawer = await screen.findByRole('dialog', { name: 'Edit Meera' });

    const name = within(drawer).getByLabelText('Name');
    await userEvent.clear(name);
    await userEvent.type(name, 'Asha');
    await userEvent.click(within(drawer).getByRole('button', { name: 'Avatar 🦉' }));
    await userEvent.click(within(drawer).getByRole('button', { name: 'Teal' }));
    await userEvent.click(within(drawer).getByRole('button', { name: 'Start from the standard guidance' }));
    expect(within(drawer).getByLabelText('Who Asha is and how it writes')).toHaveValue('Standard guidance for an HR team.');
    await userEvent.click(within(drawer).getByRole('checkbox', { name: 'Work on client requirements' }));
    await userEvent.click(within(drawer).getByRole('button', { name: 'Routine replies on its own' }));
    expect(within(drawer).getByRole('note')).toHaveTextContent(/only to someone we have written to before/);
    await userEvent.click(within(drawer).getByRole('button', { name: 'Best' }));
    await userEvent.click(within(drawer).getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(bodiesOf(fetchMock, 'PUT', '/agent')).toHaveLength(1));
    expect(bodiesOf(fetchMock, 'PUT', '/agent')[0]).toEqual({
      name: 'Asha',
      avatar: '🦉',
      color: 'teal',
      persona: 'Standard guidance for an HR team.',
      signature: 'Acme HR',
      tasks: ['reply'],
      permissions: { draft: true, send: 'auto' },
      quality: 'best',
      enabled: true,
    });
    expect(await screen.findByText('Asha is updated.')).toBeInTheDocument();
  });

  it('tests the unsaved instructions on a recent email', async () => {
    const fetchMock = mockApi('admin');
    renderAgents();
    await userEvent.click(await screen.findByRole('button', { name: 'Edit Meera' }));
    const drawer = await screen.findByRole('dialog', { name: 'Edit Meera' });
    const persona = within(drawer).getByLabelText('Who Meera is and how it writes');
    await userEvent.clear(persona);
    await userEvent.type(persona, 'Be brief.');
    await userEvent.click(within(drawer).getByRole('button', { name: 'Test on an email' }));
    const select = await within(drawer).findByLabelText('Email');
    await within(drawer).findByRole('option', { name: 'Status update — ravi@client.com' });
    await userEvent.selectOptions(select, 'm1');
    await userEvent.click(within(drawer).getByRole('button', { name: 'Write a test reply' }));

    expect(await within(drawer).findByRole('status', { name: 'Test reply' })).toHaveTextContent('Noted, thanks!');
    expect(bodiesOf(fetchMock, 'POST', '/agent/preview')[0]).toEqual({
      message_id: 'm1',
      agent: { persona: 'Be brief.', signature: 'Acme HR', quality: 'balanced' },
    });
  });
});

describe('My profile', () => {
  it('saves what the agent should know about me', async () => {
    const fetchMock = mockApi('operator');
    renderAgents();
    const card = await screen.findByRole('region', { name: 'My profile' });
    await userEvent.click(await within(card).findByRole('button', { name: 'Tell Meera about you' }));
    await userEvent.type(within(card).getByLabelText('Role or job title'), 'Senior Recruiter');
    await userEvent.click(within(within(card).getByRole('group', { name: 'What you handle' })).getByRole('checkbox', { name: 'Candidates' }));
    await userEvent.click(within(card).getByRole('checkbox', { name: 'Clients' }));
    await userEvent.type(within(card).getByLabelText('Important people or domains'), 'ceo@client.com, capgemini.com');
    await userEvent.selectOptions(within(card).getByLabelText('Formality'), 'friendly');
    await userEvent.type(within(card).getByLabelText('Sign-off'), 'Priya S.');
    await userEvent.type(within(card).getByLabelText('Things Meera must never do for you'), 'Never share salary bands');
    await userEvent.click(within(card).getByRole('button', { name: 'Let routine replies go' }));
    await userEvent.click(within(card).getByRole('button', { name: 'Save profile' }));

    await waitFor(() => expect(bodiesOf(fetchMock, 'PUT', '/me/agent-profile')).toHaveLength(1));
    const body = bodiesOf(fetchMock, 'PUT', '/me/agent-profile')[0];
    expect(body).toMatchObject({
      job_title: 'Senior Recruiter',
      handles: ['candidates'],
      audiences: ['clients'],
      vips: ['ceo@client.com', 'capgemini.com'],
      tone: { formality: 'friendly', length: '', sign_off: 'Priya S.', language: '' },
      never_do: 'Never share salary bands',
      approvals: { send_email: 'routine_ok' },
    });
  });

  it('shows admins the whole team', async () => {
    mockApi('admin');
    renderAgents();
    const table = await screen.findByRole('table', { name: 'Team profiles' });
    expect(await within(table).findByText('Priya')).toBeInTheDocument();
    expect(within(table).getByText('Recruiter')).toBeInTheDocument();
    expect(within(table).getByText('Not filled in')).toBeInTheDocument();
  });

  it('is not shown to operators as a team table', async () => {
    mockApi('operator');
    renderAgents();
    await screen.findByRole('region', { name: 'Your agent' });
    expect(screen.queryByRole('table', { name: 'Team profiles' })).toBeNull();
  });
});

describe('Onboarding', () => {
  it('asks a person who connected a mailbox to tell the agent about them, and can be skipped', async () => {
    const fetchMock = mockApi('operator', { mine: myProfile({ needs_onboarding: true }) });
    renderAgents(<OnboardingPrompt ws="w1" />);
    const dialog = await screen.findByRole('dialog', { name: 'Tell Meera about you' });
    expect(within(dialog).getByText(/You connected priya@acme.com/)).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Skip for now' }));
    await waitFor(() => expect(bodiesOf(fetchMock, 'POST', '/me/agent-profile/skip')).toHaveLength(1));
  });

  it('stays quiet when nothing is needed', async () => {
    const fetchMock = mockApi('operator');
    renderAgents(<OnboardingPrompt ws="w1" />);
    await waitFor(() => expect(fetchMock).toHaveBeenCalled());
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('Approvals name the agent', () => {
  it('shows who wants to send and for whom', async () => {
    mockApi('operator', { approvals: [approval()] });
    renderAgents();
    const card = await screen.findByRole('group', { name: 'Send reply to ravi@client.com' });
    expect(card).toHaveTextContent('Meera (agent) wants to send a reply for Priya');
  });

  it('renders a suggested automation as a sentence and creates it on approval', async () => {
    const suggestion = approval({
      id: 'ap2',
      action: 'create_automation',
      summary: 'Suggests an automation: Draft replies to status updates',
      to: null,
      subject: null,
      body: 'Status updates come in every week.',
      automation: {
        name: 'Draft replies to status updates',
        conditions: [{ field: 'subject', op: 'contains', value: 'status' }],
        actions: [{ type: 'draft_reply' }],
      },
    });
    const fetchMock = mockApi('admin', { approvals: [suggestion] });
    renderAgents();
    const card = await screen.findByRole('group', { name: 'Suggests an automation: Draft replies to status updates' });
    expect(card).toHaveTextContent('Meera (agent) suggests an automation');
    expect(within(card).getByText('contains “status”')).toBeInTheDocument();
    expect(within(card).getByText('Draft a reply')).toBeInTheDocument();
    expect(within(card).getByText('Why: Status updates come in every week.')).toBeInTheDocument();
    expect(within(card).queryByLabelText('Email body')).toBeNull();
    await userEvent.click(within(card).getByRole('button', { name: 'Create automation' }));
    await waitFor(() => expect(bodiesOf(fetchMock, 'POST', '/approvals/ap2')).toEqual([{ decision: 'approve' }]));
  });
});
