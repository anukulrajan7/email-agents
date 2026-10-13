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

  it('toggles password visibility', async () => {
    stubFetch();
    renderPage('sign-in');
    const user = userEvent.setup();
    const password = await screen.findByLabelText('Password');
    await user.type(password, 'secret-pass');
    expect(password).toHaveAttribute('type', 'password');
    await user.click(screen.getByRole('button', { name: 'Show password' }));
    expect(password).toHaveAttribute('type', 'text');
    expect(screen.getByRole('button', { name: 'Hide password' })).toHaveAttribute('aria-pressed', 'true');
    await user.click(screen.getByRole('button', { name: 'Hide password' }));
    expect(password).toHaveAttribute('type', 'password');
  });

  it('validates fields inline before calling the API', async () => {
    const fetchMock = stubFetch();
    renderPage('sign-in');
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: 'Sign in' }));
    expect(screen.getByText('Enter your email address.')).toBeInTheDocument();
    expect(screen.getByText('Enter your password.')).toBeInTheDocument();
    await user.type(screen.getByLabelText('Email'), 'not-an-email');
    await user.tab();
    expect(screen.getByText(/Enter a full email address/)).toBeInTheDocument();
    expect(screen.getByLabelText('Email')).toHaveAttribute('aria-invalid', 'true');
    expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/auth/login'))).toBe(false);
  });

  it('warns when Caps Lock is on in the password field', async () => {
    stubFetch();
    renderPage('sign-in');
    const user = userEvent.setup();
    await user.click(await screen.findByLabelText('Password'));
    expect(screen.queryByText('Caps Lock is on')).toBeNull();
    await user.keyboard('{CapsLock}a');
    expect(screen.getByText('Caps Lock is on')).toBeInTheDocument();
  });

  it('moves to the code step when signing in with an unverified email', async () => {
    stubFetch(json(403, { error: { code: 'email_not_verified', message: 'verify first' } }));
    renderPage('sign-in');
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText('Email'), 'new@b.test');
    await user.type(screen.getByLabelText('Password'), 'correct-horse');
    await user.click(screen.getByRole('button', { name: 'Sign in' }));
    expect(await screen.findByRole('heading', { name: 'Check your email' })).toBeInTheDocument();
    expect(screen.getByText(/We sent a new code to new@b.test/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Verify' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Use a different email' }));
    expect(await screen.findByRole('heading', { name: 'Sign in' })).toBeInTheDocument();
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
