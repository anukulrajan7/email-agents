import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ReplySettingsCard } from './ReplySettingsCard';

const settings = {
  auto_reply: false,
  reply_instructions: 'Warm and short.',
  signature: null,
  default_reply_style: 'We are the HR team.\n- Never reject anyone.',
};

function setup(isAdmin: boolean) {
  const fetchMock = vi.fn((_url: string, init?: RequestInit) => {
    const body = init?.method === 'PUT' ? JSON.parse(String(init.body)) : settings;
    return Promise.resolve(new Response(JSON.stringify(body)));
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ReplySettingsCard ws="w1" isAdmin={isAdmin} />
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('ReplySettingsCard', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('lets an admin change the settings and saves them with PUT', async () => {
    const fetchMock = setup(true);
    const toggle = await screen.findByRole('switch', { name: 'Draft replies automatically' });
    expect(toggle).not.toBeChecked();
    expect(screen.getByText(/drafts are written only when you click Generate draft/)).toBeInTheDocument();
    const save = screen.getByRole('button', { name: 'Save' });
    expect(save).toBeDisabled();

    await userEvent.click(toggle);
    await userEvent.type(screen.getByLabelText('How should replies sound?'), ' Mention 1 working day.');
    await userEvent.type(screen.getByLabelText('Signature'), 'The Acme Team');
    await userEvent.click(save);

    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/reply-settings',
        expect.objectContaining({
          method: 'PUT',
          body: JSON.stringify({
            auto_reply: true,
            reply_instructions: 'Warm and short. Mention 1 working day.',
            signature: 'The Acme Team',
          }),
        }),
      ),
    );
  });

  it('shows the standard guidance and can use it as a starting point', async () => {
    setup(true);
    await userEvent.click(await screen.findByText('See the standard guidance'));
    expect(screen.getAllByText(/We are the HR team\./)[0]).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Use as a starting point' }));
    expect(screen.getByLabelText('How should replies sound?')).toHaveValue(settings.default_reply_style);
  });

  it('is read-only for operators', async () => {
    setup(false);
    expect(await screen.findByRole('switch', { name: 'Draft replies automatically' })).toBeDisabled();
    expect(screen.getByLabelText('How should replies sound?')).toHaveAttribute('readonly');
    expect(screen.getByLabelText('Signature')).toHaveAttribute('readonly');
    expect(screen.queryByRole('button', { name: 'Save' })).toBeNull();
    expect(screen.getByText('Only a workspace admin can change these.')).toBeInTheDocument();
  });

  it('never mentions model names', async () => {
    setup(true);
    await screen.findByRole('region', { name: 'Replies' });
    expect(screen.getByRole('region', { name: 'Replies' }).textContent).not.toMatch(/gpt|claude|kimi|deepseek|haiku|openrouter|premium|model/i);
  });
});
