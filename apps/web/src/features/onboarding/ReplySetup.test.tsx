import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ReplySetup } from './ReplySetup';

const preset = (key: string, name: string) => ({
  key,
  name,
  description: `${name} description`,
  automation: { name, enabled: true, conditions: [{ field: 'needs_reply', op: 'is', value: true }], actions: [{ type: 'draft_reply' }] },
});

const json = (body: unknown, status = 200) => Promise.resolve(new Response(JSON.stringify(body), { status }));

function setup() {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    const method = init?.method ?? 'GET';
    if (url.endsWith('/me')) {
      return json({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role: 'admin', created_at: '' }], auth_mode: 'password' });
    }
    if (url.endsWith('/automations/presets')) return json({ items: [preset('reply_to_questions', 'Reply to questions'), preset('auto_ack_referrals', 'Acknowledge referrals')] });
    if (url.endsWith('/automations/a1') && method === 'PATCH') return json({});
    if (url.endsWith('/automations/presets/auto_ack_referrals') && method === 'POST') return json({}, 201);
    if (url.endsWith('/automations')) {
      return json({ items: [{ id: 'a1', preset_key: 'reply_to_questions', name: 'Reply to questions', enabled: true, conditions: [], actions: [] }] });
    }
    if (url.endsWith('/labels/suggest')) return json({ items: [] });
    if (url.includes('/labels')) return json({ items: [] });
    return json({});
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter>
        <ReplySetup ws="w1" />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  const calls = () => fetchMock.mock.calls.map(([u, i]) => `${i?.method ?? 'GET'} ${u}`);
  return { calls, user: userEvent.setup() };
}

describe('ReplySetup', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.unstubAllGlobals());

  it('shows seeded presets as on, toggles via PATCH, and creates the others from the preset', async () => {
    const { calls, user } = setup();
    expect(await screen.findByRole('heading', { name: 'How should we handle replies?' })).toBeInTheDocument();
    const seeded = await screen.findByRole('switch', { name: 'Reply to questions' });
    await waitFor(() => expect(seeded).toBeChecked());
    const other = screen.getByRole('switch', { name: 'Acknowledge referrals' });
    expect(other).not.toBeChecked();

    await user.click(seeded);
    await waitFor(() => expect(calls()).toContain('PATCH /api/v1/workspaces/w1/automations/a1'));
    await user.click(other);
    await waitFor(() => expect(calls()).toContain('POST /api/v1/workspaces/w1/automations/presets/auto_ack_referrals'));
  });

  it('moves to the label step and stays hidden once finished', async () => {
    const { user } = setup();
    await user.click(await screen.findByRole('button', { name: 'Next' }));
    expect(screen.getByRole('heading', { name: 'Sort your mail into labels' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Done' }));
    expect(screen.queryByRole('region')).toBeNull();
    expect(localStorage.getItem('getting-started:w1:reply-setup-done')).toBe('1');
  });

  it('can be skipped', async () => {
    const { user } = setup();
    await user.click(await screen.findByRole('button', { name: 'Skip for now' }));
    expect(screen.queryByRole('region')).toBeNull();
  });
});
