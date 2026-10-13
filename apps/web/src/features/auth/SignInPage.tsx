import { useEffect, useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { ApiError, setToken } from '../../api/client';
import { Button, Card, Input, Label, Spinner } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { riseStyle } from '../../lib/motion';
import { useAuthConfig, useLogin, useResendCode, useSignup, useVerify } from './hooks';

const MIN_PASSWORD = 10;
const RESEND_COOLDOWN = 60;
const linkClass = 'font-semibold text-accent underline underline-offset-2 hover:text-accent-hover';

function authErrorMessage(error: unknown, mode: 'sign-in' | 'sign-up'): Error | undefined {
  if (!error) return undefined;
  if (error instanceof ApiError) {
    if (error.status === 401) return new Error('Email or password is incorrect');
    if (error.status === 409) return new Error('An account with this email already exists — sign in instead');
    if (error.status === 429) return new Error('Too many attempts. Try again in a few minutes.');
    if (error.status === 403 && mode === 'sign-up') return new Error("Sign-ups are restricted for this workspace's domain.");
  }
  return error instanceof Error ? error : new Error('Something went wrong');
}

function verifyErrorMessage(error: unknown): Error | undefined {
  if (!error) return undefined;
  if (error instanceof ApiError) {
    if (error.status === 400) return new Error('That code is invalid or expired.');
    if (error.status === 429) return new Error('Too many attempts. Try again in a few minutes.');
  }
  return error instanceof Error ? error : new Error('Something went wrong');
}

function VerifyStep({ email, notice, onBack }: { email: string; notice?: string; onBack: () => void }) {
  const [code, setCode] = useState('');
  const [cooldown, setCooldown] = useState(RESEND_COOLDOWN);
  const verify = useVerify();
  const resend = useResendCode();

  useEffect(() => {
    if (cooldown <= 0) return;
    const id = window.setTimeout(() => setCooldown((c) => c - 1), 1000);
    return () => window.clearTimeout(id);
  }, [cooldown]);

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (verify.isPending || code.length !== 6) return;
    verify.mutate({ email, code });
  };

  const sendAgain = () => {
    resend.mutate(email, { onSuccess: () => setCooldown(RESEND_COOLDOWN) });
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <div>
        <p className="kicker">One more step</p>
        <h2 className="type-display mt-1 text-3xl tracking-[-0.02em]">Check your email</h2>
        <p className="mt-2 text-sm text-ink-muted">
          {notice ?? `We sent a 6-digit code to ${email}.`} It expires in 10 minutes.
        </p>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="otp-code">Verification code</Label>
        <Input
          id="otp-code"
          inputMode="numeric"
          autoComplete="one-time-code"
          maxLength={6}
          required
          autoFocus
          placeholder="123456"
          className="font-mono tracking-[0.4em]"
          value={code}
          onChange={(e) => setCode(e.target.value.replace(/\D/g, ''))}
        />
      </div>
      <ErrorNote error={verifyErrorMessage(verify.error) ?? resend.error} />
      <Button type="submit" loading={verify.isPending} disabled={code.length !== 6}>
        Verify
      </Button>
      <div className="flex flex-wrap items-center justify-between gap-2 border-t border-rule pt-4 text-sm">
        <Button variant="secondary" size="sm" loading={resend.isPending} disabled={cooldown > 0} onClick={sendAgain}>
          {cooldown > 0 ? `Resend code (${cooldown}s)` : 'Resend code'}
        </Button>
        <button type="button" className={linkClass} onClick={onBack}>
          Use a different email
        </button>
      </div>
      {resend.isSuccess && cooldown > 0 && (
        <p role="status" className="text-xs text-ink-muted">
          A new code is on its way.
        </p>
      )}
    </form>
  );
}

function DevForm() {
  const [email, setEmail] = useState('');
  const navigate = useNavigate();
  const qc = useQueryClient();

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = email.trim();
    if (!trimmed) return;
    setToken(`dev:${trimmed}`);
    qc.clear();
    navigate('/workspaces');
  };

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <div>
        <p className="kicker">Dev sign-in</p>
        <h2 className="type-display mt-1 text-3xl tracking-[-0.02em]">Sign in</h2>
        <p className="mt-2 text-sm text-ink-muted">Enter any email to continue.</p>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="email">Email</Label>
        <Input id="email" type="email" required autoFocus autoComplete="email" placeholder="admin@acme.test" value={email} onChange={(e) => setEmail(e.target.value)} />
      </div>
      <Button type="submit">Continue</Button>
      <p className="border-t border-rule pt-4 text-xs text-ink-muted">
        OIDC mode is configured server-side. This form only works when the API runs with{' '}
        <code className="font-mono">AUTH_MODE=dev</code>.
      </p>
    </form>
  );
}

function PasswordForm({ mode, signupEnabled }: { mode: 'sign-in' | 'sign-up'; signupEnabled: boolean }) {
  const isSignUp = mode === 'sign-up';
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [show, setShow] = useState(false);
  const [pending, setPending] = useState<{ email: string; notice?: string } | undefined>();
  const login = useLogin();
  const signup = useSignup();
  const action = isSignUp ? signup : login;
  const tooShort = isSignUp && password.length < MIN_PASSWORD;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (action.isPending || tooShort || !email.trim() || !password) return;
    if (isSignUp) {
      const trimmedName = name.trim();
      signup.mutate(
        { email: email.trim(), password, ...(trimmedName ? { name: trimmedName } : {}) },
        { onSuccess: (res) => 'verification_required' in res && setPending({ email: res.email || email.trim() }) },
      );
    } else {
      const trimmedEmail = email.trim();
      login.mutate(
        { email: trimmedEmail, password },
        {
          onError: (err) => {
            if (err instanceof ApiError && err.status === 403 && err.code === 'email_not_verified') {
              setPending({ email: trimmedEmail, notice: `We sent a new code to ${trimmedEmail}.` });
            }
          },
        },
      );
    }
  };

  if (pending) {
    return <VerifyStep email={pending.email} notice={pending.notice} onBack={() => { setPending(undefined); setPassword(''); }} />;
  }

  const notVerified = action.error instanceof ApiError && action.error.code === 'email_not_verified';
  const error = notVerified ? undefined : authErrorMessage(action.error, mode);
  const conflict = action.error instanceof ApiError && action.error.status === 409;

  return (
    <form onSubmit={submit} className="flex flex-col gap-5">
      <div>
        <p className="kicker">{isSignUp ? 'New account' : 'Welcome back'}</p>
        <h2 className="type-display mt-1 text-3xl tracking-[-0.02em]">{isSignUp ? 'Create account' : 'Sign in'}</h2>
      </div>
      {isSignUp && (
        <div className="flex flex-col gap-1">
          <Label htmlFor="name">Name (optional)</Label>
          <Input id="name" autoComplete="name" maxLength={100} value={name} onChange={(e) => setName(e.target.value)} />
        </div>
      )}
      <div className="flex flex-col gap-1">
        <Label htmlFor="email">Email</Label>
        <Input id="email" type="email" required autoFocus autoComplete="email" placeholder="you@company.com" value={email} onChange={(e) => setEmail(e.target.value)} />
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="password">Password</Label>
        <div className="flex gap-2">
          <Input
            id="password"
            type={show ? 'text' : 'password'}
            required
            autoComplete={isSignUp ? 'new-password' : 'current-password'}
            maxLength={128}
            aria-describedby={isSignUp ? 'password-hint' : undefined}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          <Button variant="secondary" aria-pressed={show} aria-label={show ? 'Hide password' : 'Show password'} onClick={() => setShow((v) => !v)}>
            {show ? 'Hide' : 'Show'}
          </Button>
        </div>
        {isSignUp && (
          <p id="password-hint" className={tooShort && password.length > 0 ? 'text-xs text-danger' : 'text-xs text-ink-muted'}>
            {tooShort ? `${password.length}/${MIN_PASSWORD} — use at least ${MIN_PASSWORD} characters` : 'Looks good'}
          </p>
        )}
      </div>
      <ErrorNote error={error} />
      {conflict && (
        <Link to="/sign-in" className={linkClass}>
          Go to sign in
        </Link>
      )}
      <Button type="submit" loading={action.isPending} disabled={tooShort}>
        {isSignUp ? 'Create account' : 'Sign in'}
      </Button>
      {signupEnabled || isSignUp ? (
        <p className="border-t border-rule pt-4 text-sm text-ink-muted">
          {isSignUp ? (
            <>
              Already have an account?{' '}
              <Link to="/sign-in" className={linkClass}>
                Sign in
              </Link>
            </>
          ) : (
            <>
              New here?{' '}
              <Link to="/sign-up" className={linkClass}>
                Create an account
              </Link>
            </>
          )}
        </p>
      ) : null}
    </form>
  );
}

export function SignInPage({ mode }: { mode: 'sign-in' | 'sign-up' }) {
  const config = useAuthConfig();
  const signupEnabled = config.data?.signup_enabled ?? true;
  const effectiveMode = mode === 'sign-up' && signupEnabled ? 'sign-up' : 'sign-in';

  return (
    <div className="flex min-h-dvh flex-col">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <div className="grid w-full flex-1 items-center gap-12 px-8 py-10 lg:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)] lg:px-16">
        <div className="rise" style={riseStyle(0)}>
          <p className="kicker">AI Inbox · Post Room</p>
          <h1 className="type-display mt-3 text-5xl leading-[1.05] tracking-[-0.025em] text-ink xl:text-6xl">
            Every email, sorted and answered{' '}
            <span className="text-accent">— by you.</span>
          </h1>
          <p className="mt-6 max-w-prose text-[17px] text-ink-muted">
            Connect or forward your email. The AI sorts each one into labels you define and drafts a reply. Nothing is ever sent automatically.
          </p>
        </div>
        <Card className="rise w-full max-w-sm justify-self-center bg-raised p-8 lg:justify-self-end" style={riseStyle(2)}>
          {config.isLoading ? (
            <div className="flex justify-center py-8 text-ink-muted">
              <Spinner />
            </div>
          ) : config.data?.mode === 'dev' ? (
            <DevForm />
          ) : (
            <PasswordForm key={effectiveMode} mode={effectiveMode} signupEnabled={signupEnabled} />
          )}
        </Card>
      </div>
    </div>
  );
}
