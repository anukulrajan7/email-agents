import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { SignInPage } from './SignInPage';

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

function renderSignIn() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={['/sign-in']}>
        <SignInPage mode="sign-in" />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe('ResetPassword', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('requests a code, sets a new password and returns to sign-in', async () => {
    const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
      if (url.endsWith('/auth/config')) return Promise.resolve(json(200, { mode: 'password', signup_enabled: true }));
      if (url.endsWith('/auth/password-reset/request')) return Promise.resolve(json(200, { status: 'sent' }));
      if (url.endsWith('/auth/password-reset/confirm')) return Promise.resolve(json(200, { status: 'password_reset' }));
      return Promise.resolve(json(204, {}));
    });
    vi.stubGlobal('fetch', fetchMock);
    renderSignIn();
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText('Email'), 'a@b.test');
    await user.click(screen.getByRole('button', { name: 'Forgot password?' }));

    expect(screen.getByRole('heading', { name: 'Reset your password' })).toBeInTheDocument();
    expect(screen.getByLabelText('Email')).toHaveValue('a@b.test');
    await user.click(screen.getByRole('button', { name: 'Send code' }));

    await user.type(await screen.findByLabelText('Code'), '123456');
    await user.type(screen.getByLabelText('New password'), 'short');
    expect(screen.getByRole('button', { name: 'Set new password' })).toBeDisabled();
    await user.type(screen.getByLabelText('New password'), '-but-longer');
    await user.click(screen.getByRole('button', { name: 'Set new password' }));

    expect(await screen.findByText('Password changed. Sign in with your new password.')).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Sign in' })).toBeInTheDocument();
    const confirmCall = fetchMock.mock.calls.find(([u]) => u.endsWith('/auth/password-reset/confirm'));
    expect(JSON.parse(confirmCall?.[1]?.body as string)).toEqual({ email: 'a@b.test', code: '123456', new_password: 'short-but-longer' });
  });

  it('explains an invalid code', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string) => {
        if (url.endsWith('/auth/config')) return Promise.resolve(json(200, { mode: 'password', signup_enabled: true }));
        if (url.endsWith('/auth/password-reset/request')) return Promise.resolve(json(200, { status: 'sent' }));
        return Promise.resolve(json(400, { error: { code: 'bad_request', message: 'invalid or expired code' } }));
      }),
    );
    renderSignIn();
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: 'Forgot password?' }));
    await user.type(screen.getByLabelText('Email'), 'a@b.test');
    await user.click(screen.getByRole('button', { name: 'Send code' }));
    await user.type(await screen.findByLabelText('Code'), '000000');
    await user.type(screen.getByLabelText('New password'), 'long enough password');
    await user.click(screen.getByRole('button', { name: 'Set new password' }));
    expect(await screen.findByText('That code is invalid or expired.')).toBeInTheDocument();
  });
});
