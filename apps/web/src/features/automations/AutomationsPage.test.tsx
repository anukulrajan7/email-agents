import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AutomationsPage } from './AutomationsPage';
import type { Automation, Preset } from './types';

const automation: Automation = {
  id: 'a1',
  name: 'Timesheet replies',
  enabled: true,
  trigger: 'email_received',
  preset_key: null,
  conditions: [
    { field: 'needs_reply', op: 'is', value: true },
    { field: 'label', op: 'is', value: 'timesheet' },
  ],
  actions: [{ type: 'draft_reply' }, { type: 'save_to_gmail_drafts' }],
  run_count: 3,
  last_run_at: new Date(Date.now() - 2 * 3600_000).toISOString(),
  created_at: '',
  updated_at: '',
};

const presets: Preset[] = [
  {
    key: 'reply_to_questions',
    name: 'Reply to questions',
    description: 'Draft an answer whenever someone asks you something.',
    automation: {
      name: 'Reply to questions',
      enabled: true,
      conditions: [{ field: 'needs_reply', op: 'is', value: true }],
      actions: [{ type: 'draft_reply' }, { type: 'save_to_gmail_drafts' }],
    },
  },
];

const labels = [
  { id: 'l1', key: 'timesheet', name: 'Timesheet', description: 'Weekly timesheets', color: 'slate', position: 1, system: false, archived: false, message_count: 1 },
];

const json = (body: unknown, status = 200) => Promise.resolve(new Response(status === 204 ? null : JSON.stringify(body), { status }));

function setup() {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    const method = init?.method ?? 'GET';
    if (url.endsWith('/me')) {
      return json({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role: 'admin', created_at: '' }], auth_mode: 'password' });
    }
    if (url.includes('/labels')) return json({ items: labels });
    if (url.endsWith('/automations/presets') && method === 'GET') return json({ items: presets });
    if (url.endsWith('/automations/presets/reply_to_questions') && method === 'POST') return json({ ...automation, id: 'a2', preset_key: 'reply_to_questions' }, 201);
    if (url.includes('/automations/a1/runs')) {
      return json({
        items: [
          {
            id: 'r1',
            automation_id: 'a1',
            message_id: 'm1',
            message_subject: 'Timesheet week 40',
            status: 'completed',
            actions: [
              { type: 'draft_reply', status: 'done', detail: null },
              { type: 'save_to_gmail_drafts', status: 'failed', detail: 'Gmail is disconnected' },
            ],
            created_at: new Date().toISOString(),
          },
        ],
      });
    }
    if (url.endsWith('/automations/a1') && method === 'DELETE') return json(null, 204);
    if (url.endsWith('/automations/a1') && method === 'PATCH') return json({ ...automation, enabled: false });
    if (url.endsWith('/automations')) return json({ items: [automation] });
    return json({});
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/automations']}>
        <Routes>
          <Route path="/w/:ws/automations" element={<AutomationsPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  const calls = () => fetchMock.mock.calls.map(([u, i]) => `${i?.method ?? 'GET'} ${u}`);
  return { fetchMock, calls, user: userEvent.setup() };
}

describe('AutomationsPage', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('shows each automation as a readable sentence with run info', async () => {
    setup();
    const list = await screen.findByRole('list', { name: 'Automations' });
    expect(list).toHaveTextContent('When an email needs a reply and label is Timesheet');
    expect(list).toHaveTextContent('Draft a reply, Save to Gmail drafts');
    expect(list).toHaveTextContent('Ran 3 times');
    expect(screen.getByRole('switch', { name: 'Timesheet replies enabled' })).toBeChecked();
  });

  it('toggles an automation with PATCH', async () => {
    const { calls, user } = setup();
    await user.click(await screen.findByRole('switch', { name: 'Timesheet replies enabled' }));
    await waitFor(() => expect(calls()).toContain('PATCH /api/v1/workspaces/w1/automations/a1'));
  });

  it('creates an automation from a preset with one click and renders what the API returned', async () => {
    const { calls, user } = setup();
    const gallery = (await screen.findByRole('heading', { name: 'Start from a preset' })).parentElement as HTMLElement;
    expect(await within(gallery).findByText('Draft an answer whenever someone asks you something.')).toBeInTheDocument();
    await user.click(within(gallery).getByRole('button', { name: 'Use this preset: Reply to questions' }));
    await waitFor(() => expect(calls()).toContain('POST /api/v1/workspaces/w1/automations/presets/reply_to_questions'));
  });

  it('confirms before deleting', async () => {
    const { calls, user } = setup();
    await user.click(await screen.findByRole('button', { name: 'Delete Timesheet replies' }));
    const dialog = screen.getByRole('dialog');
    expect(calls()).not.toContain('DELETE /api/v1/workspaces/w1/automations/a1');
    await user.click(within(dialog).getByRole('button', { name: 'Delete' }));
    await waitFor(() => expect(calls()).toContain('DELETE /api/v1/workspaces/w1/automations/a1'));
  });

  it('opens the runs drawer with per-action status', async () => {
    const { user } = setup();
    await user.click(await screen.findByRole('button', { name: 'Runs for Timesheet replies' }));
    const drawer = await screen.findByRole('dialog');
    expect(await within(drawer).findByText('Timesheet week 40')).toBeInTheDocument();
    expect(drawer).toHaveTextContent('Gmail is disconnected');
    expect(drawer).toHaveTextContent('failed');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});
