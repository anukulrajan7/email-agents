import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { TOKEN_KEY } from '../../api/client';
import { SignInPage } from './SignInPage';

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

function renderPage(mode: 'sign-in' | 'sign-up') {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={qc}>
      <MemoryRouter initialEntries={[mode === 'sign-in' ? '/sign-in' : '/sign-up']}>
        <SignInPage mode={mode} />
        <Where />
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

function stubFetch(login?: Response) {
  const fetchMock = vi.fn((url: string) =>
    Promise.resolve(url.endsWith('/auth/config') ? json(200, { mode: 'password', signup_enabled: true }) : (login ?? json(204, {}))),
  );
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

describe('SignInPage', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.unstubAllGlobals());

  it('shows a friendly message on login 401 without redirecting', async () => {
    const assign = vi.fn();
    vi.stubGlobal('location', { pathname: '/sign-in', assign });
    stubFetch(json(401, { error: { code: 'unauthorized', message: 'invalid email or password' } }));
    renderPage('sign-in');
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText('Email'), 'a@b.test');
    await user.type(screen.getByLabelText('Password'), 'wrong-password');
    await user.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(await screen.findByText('Email or password is incorrect')).toBeInTheDocument();
    expect(assign).not.toHaveBeenCalled();
    expect(screen.getByTestId('where')).toHaveTextContent('/sign-in');
  });

  it('disables sign-up submit until the password has 10 characters', async () => {
    stubFetch();
    renderPage('sign-up');
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText('Email'), 'a@b.test');
    await user.type(screen.getByLabelText('Password'), 'short');
    expect(screen.getByRole('button', { name: 'Create account' })).toBeDisabled();
    await user.type(screen.getByLabelText('Password'), '12345');
    expect(screen.getByRole('button', { name: 'Create account' })).toBeEnabled();
  });

  it('stores the token after a successful login', async () => {
    stubFetch(json(200, { token: 'tok_123', expires_at: '2030-01-01T00:00:00Z', user: { id: 'u1', email: 'a@b.test', name: null } }));
    renderPage('sign-in');
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText('Email'), 'a@b.test');
    await user.type(screen.getByLabelText('Password'), 'correct-horse');
    await user.click(screen.getByRole('button', { name: 'Sign in' }));
    await waitFor(() => expect(localStorage.getItem(TOKEN_KEY)).toBe('tok_123'));
    await waitFor(() => expect(screen.getByTestId('where')).toHaveTextContent('/workspaces'));
  });

  it('shows the code step after a 202 sign-up and stores the token on verify', async () => {
    const fetchMock = vi.fn((url: string, _init?: RequestInit) => {
      if (url.endsWith('/auth/config')) return Promise.resolve(json(200, { mode: 'password', signup_enabled: true, email_verification: true }));
      if (url.endsWith('/auth/signup')) return Promise.resolve(json(202, { verification_required: true, email: 'a@b.test' }));
      if (url.endsWith('/auth/verify'))
        return Promise.resolve(json(200, { token: 'tok_otp', expires_at: '2030-01-01T00:00:00Z', user: { id: 'u1', email: 'a@b.test', name: null } }));
      return Promise.resolve(json(204, {}));
    });
    vi.stubGlobal('fetch', fetchMock);
    renderPage('sign-up');
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText('Email'), 'a@b.test');
    await user.type(screen.getByLabelText('Password'), 'correct-horse');
    await user.click(screen.getByRole('button', { name: 'Create account' }));
    expect(await screen.findByRole('heading', { name: 'Check your email' })).toBeInTheDocument();
    expect(localStorage.getItem(TOKEN_KEY)).toBeNull();
    await user.type(screen.getByLabelText('Verification code'), '123456');
    await user.click(screen.getByRole('button', { name: 'Verify' }));
    await waitFor(() => expect(localStorage.getItem(TOKEN_KEY)).toBe('tok_otp'));
    const verifyCall = fetchMock.mock.calls.find(([u]) => u.endsWith('/auth/verify'));
    expect(JSON.parse(verifyCall?.[1]?.body as string)).toEqual({ email: 'a@b.test', code: '123456' });
  });
});
