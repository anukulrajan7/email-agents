import { useEffect, useState, type FormEvent, type KeyboardEvent, type ReactNode } from 'react';
import { Link, useNavigate } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { ApiError, setToken } from '../../api/client';
import { Button, Input, Label, Skeleton } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { riseStyle } from '../../lib/motion';
import { BrandPanel } from './BrandPanel';
import { useAuthConfig, useLogin, useResendCode, useSignup, useVerify } from './hooks';

const MIN_PASSWORD = 10;
const RESEND_COOLDOWN = 60;
const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const linkClass = 'rounded-xs font-semibold text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover';

type Mode = 'sign-in' | 'sign-up';

function authErrorMessage(error: unknown, mode: Mode): Error | undefined {
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

function AuthFrame({
  kicker,
  title,
  subtitle,
  footer,
  children,
}: {
  kicker: string;
  title: string;
  subtitle?: ReactNode;
  footer?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="rise w-full max-w-[25rem]" style={riseStyle(1)}>
      <p className="kicker">{kicker}</p>
      <h1 className="type-display mt-2 text-[2.25rem] leading-[1.05] tracking-[-0.025em] text-ink">{title}</h1>
      {subtitle && <p className="mt-2 text-[15px] leading-relaxed text-ink-muted">{subtitle}</p>}
      <div className="mt-6 rounded-sm border border-rule-strong bg-raised p-6 shadow-[0_1px_0_var(--color-rule),0_24px_40px_-32px_rgba(28,27,24,0.5)] sm:p-7">
        {children}
      </div>
      {footer && <div className="mt-5 text-sm text-ink-muted">{footer}</div>}
    </div>
  );
}

function FieldHint({ id, tone = 'muted', children }: { id: string; tone?: 'muted' | 'danger' | 'warning' | 'success'; children: ReactNode }) {
  return (
    <p
      id={id}
      className={cn(
        'text-xs',
        tone === 'danger' && 'text-danger',
        tone === 'warning' && 'font-semibold text-review-ink',
        tone === 'success' && 'text-success',
        tone === 'muted' && 'text-ink-muted',
      )}
    >
      {children}
    </p>
  );
}

function VerifyStep({ email, notice, isSignUp, onBack }: { email: string; notice?: string; isSignUp: boolean; onBack: () => void }) {
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
    <AuthFrame
      kicker={isSignUp ? 'Step 2 of 2 · Verify' : 'One more step'}
      title="Check your email"
      subtitle={
        <>
          {notice ?? (
            <>
              We sent a 6-digit code to <span className="font-semibold break-all text-ink">{email}</span>.
            </>
          )}{' '}
          It expires in 10 minutes.
        </>
      }
      footer={
        <button type="button" className={linkClass} onClick={onBack}>
          Use a different email
        </button>
      }
    >
      <form onSubmit={submit} className="flex flex-col gap-5" noValidate>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="otp-code">Verification code</Label>
          <Input
            id="otp-code"
            inputMode="numeric"
            autoComplete="one-time-code"
            maxLength={6}
            required
            autoFocus
            placeholder="••••••"
            aria-describedby="otp-hint"
            className="h-14 text-center font-mono text-2xl tracking-[0.5em] placeholder:tracking-[0.5em]"
            value={code}
            onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
          />
          <FieldHint id="otp-hint">{code.length === 6 ? 'Ready to verify.' : `${6 - code.length} digits to go`}</FieldHint>
        </div>
        <ErrorNote error={verifyErrorMessage(verify.error) ?? resend.error} />
        <Button type="submit" className="h-11 w-full" loading={verify.isPending} disabled={code.length !== 6}>
          Verify
        </Button>
        <div className="flex flex-wrap items-center justify-between gap-2 border-t border-rule pt-4">
          <p className="text-sm text-ink-muted">No email yet?</p>
          <Button variant="secondary" size="sm" loading={resend.isPending} disabled={cooldown > 0} onClick={sendAgain}>
            {cooldown > 0 ? `Resend code (${cooldown}s)` : 'Resend code'}
          </Button>
        </div>
        {resend.isSuccess && cooldown > 0 && (
          <p role="status" className="text-xs text-success">
            A new code is on its way.
          </p>
        )}
      </form>
    </AuthFrame>
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
    <AuthFrame kicker="Developer sign-in" title="Sign in" subtitle="Enter any email to continue.">
      <form onSubmit={submit} className="flex flex-col gap-5">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="email">Email</Label>
          <Input id="email" type="email" required autoFocus autoComplete="email" placeholder="admin@acme.test" value={email} onChange={(e) => setEmail(e.target.value)} />
        </div>
        <Button type="submit" className="h-11 w-full">
          Continue
        </Button>
        <p className="border-t border-rule pt-4 text-xs text-ink-muted">
          This form only works when the server runs in developer mode (<code className="font-mono">AUTH_MODE=dev</code>).
        </p>
      </form>
    </AuthFrame>
  );
}

function PasswordForm({ mode, signupEnabled }: { mode: Mode; signupEnabled: boolean }) {
  const isSignUp = mode === 'sign-up';
  const [name, setName] = useState('');
  const [email, setEmail] = useState('');
  const [emailTouched, setEmailTouched] = useState(false);
  const [password, setPassword] = useState('');
  const [show, setShow] = useState(false);
  const [capsOn, setCapsOn] = useState(false);
  const [pending, setPending] = useState<{ email: string; notice?: string } | undefined>();
  const login = useLogin();
  const signup = useSignup();
  const action = isSignUp ? signup : login;
  const tooShort = isSignUp && password.length < MIN_PASSWORD;
  const trimmedEmail = email.trim();
  const [submitted, setSubmitted] = useState(false);
  const emailError =
    emailTouched && trimmedEmail !== '' && !EMAIL_RE.test(trimmedEmail)
      ? 'Enter a full email address, like name@company.com.'
      : submitted && trimmedEmail === ''
        ? 'Enter your email address.'
        : undefined;
  const emailInvalid = emailError !== undefined;
  const passwordMissing = submitted && password === '';

  const readCaps = (e: KeyboardEvent<HTMLInputElement>) => setCapsOn(e.getModifierState('CapsLock'));

  const submit = (e: FormEvent) => {
    e.preventDefault();
    setEmailTouched(true);
    setSubmitted(true);
    if (action.isPending || tooShort || !trimmedEmail || !password) return;
    if (!EMAIL_RE.test(trimmedEmail)) return;
    if (isSignUp) {
      const trimmedName = name.trim();
      signup.mutate(
        { email: trimmedEmail, password, ...(trimmedName ? { name: trimmedName } : {}) },
        { onSuccess: (res) => 'verification_required' in res && setPending({ email: res.email || trimmedEmail }) },
      );
    } else {
      login.mutate(
        { email: trimmedEmail, password },
        {
          onError: (err) => {
            if (err instanceof ApiError && err.status === 403 && err.code === 'email_not_verified') {
              setPending({ email: trimmedEmail, notice: `Your email isn't verified yet. We sent a new code to ${trimmedEmail}.` });
            }
          },
        },
      );
    }
  };

  if (pending) {
    return (
      <VerifyStep
        email={pending.email}
        notice={pending.notice}
        isSignUp={isSignUp}
        onBack={() => {
          action.reset();
          setPending(undefined);
          setPassword('');
        }}
      />
    );
  }

  const notVerified = action.error instanceof ApiError && action.error.code === 'email_not_verified';
  const error = notVerified ? undefined : authErrorMessage(action.error, mode);
  const conflict = action.error instanceof ApiError && action.error.status === 409;
  const passwordHints = [passwordMissing ? 'password-error' : undefined, isSignUp ? 'password-hint' : undefined, capsOn ? 'caps-hint' : undefined].filter(Boolean).join(' ') || undefined;

  const footer =
    signupEnabled || isSignUp ? (
      isSignUp ? (
        <>
          Already have an account?{' '}
          <Link to="/sign-in" className={linkClass}>
            Sign in
          </Link>
        </>
      ) : (
        <>
          New to Post Room?{' '}
          <Link to="/sign-up" className={linkClass}>
            Create an account
          </Link>
        </>
      )
    ) : undefined;

  return (
    <AuthFrame
      kicker={isSignUp ? 'Step 1 of 2 · New account' : 'Welcome back'}
      title={isSignUp ? 'Create account' : 'Sign in'}
      subtitle={isSignUp ? 'Set up your desk in a minute. We will email you a code to confirm it is you.' : 'Pick up where your inbox left off.'}
      footer={footer}
    >
      <form onSubmit={submit} className="flex flex-col gap-5" noValidate>
        {isSignUp && (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="name">
              Name <span className="font-normal tracking-normal normal-case">(optional)</span>
            </Label>
            <Input id="name" autoComplete="name" maxLength={100} placeholder="Priya Sharma" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
        )}
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="email">Email</Label>
          <Input
            id="email"
            type="email"
            required
            autoFocus
            autoComplete="email"
            placeholder="you@company.com"
            aria-invalid={emailInvalid || undefined}
            aria-describedby={emailInvalid ? 'email-error' : undefined}
            className={cn(emailInvalid && 'border-danger hover:border-danger')}
            value={email}
            onChange={(e) => setEmail(e.target.value)}
            onBlur={() => setEmailTouched(true)}
          />
          {emailInvalid && (
            <FieldHint id="email-error" tone="danger">
              {emailError}
            </FieldHint>
          )}
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="password">Password</Label>
          <div className="relative">
            <Input
              id="password"
              type={show ? 'text' : 'password'}
              required
              autoComplete={isSignUp ? 'new-password' : 'current-password'}
              maxLength={128}
              aria-describedby={passwordHints}
              aria-invalid={passwordMissing || undefined}
              className={cn('pr-20', passwordMissing && 'border-danger hover:border-danger')}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={readCaps}
              onKeyUp={readCaps}
              onBlur={() => setCapsOn(false)}
            />
            <button
              type="button"
              aria-pressed={show}
              aria-label={show ? 'Hide password' : 'Show password'}
              onClick={() => setShow((v) => !v)}
              className="absolute inset-y-1 right-1 rounded-xs px-3 text-[13px] font-semibold text-ink-muted hover:bg-ink/5 hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
            >
              {show ? 'Hide' : 'Show'}
            </button>
          </div>
          {passwordMissing && (
            <FieldHint id="password-error" tone="danger">
              Enter your password.
            </FieldHint>
          )}
          {capsOn && (
            <FieldHint id="caps-hint" tone="warning">
              Caps Lock is on
            </FieldHint>
          )}
          {isSignUp && (
            <div className="flex flex-col gap-1">
              <span aria-hidden="true" className="flex gap-1">
                {Array.from({ length: 5 }, (_, i) => (
                  <span
                    key={i}
                    className={cn(
                      'h-1 flex-1 rounded-full motion-safe:transition-colors',
                      password.length >= (i + 1) * (MIN_PASSWORD / 5) ? (tooShort ? 'bg-review' : 'bg-success') : 'bg-rule',
                    )}
                  />
                ))}
              </span>
              <FieldHint id="password-hint" tone={tooShort ? (password.length > 0 ? 'danger' : 'muted') : 'success'}>
                {tooShort ? `${password.length}/${MIN_PASSWORD} — use at least ${MIN_PASSWORD} characters` : 'Looks good'}
              </FieldHint>
            </div>
          )}
        </div>
        <ErrorNote error={error} />
        {conflict && (
          <Link to="/sign-in" className={cn(linkClass, 'text-sm')}>
            Go to sign in
          </Link>
        )}
        <Button type="submit" className="h-11 w-full" loading={action.isPending} disabled={tooShort}>
          {isSignUp ? 'Create account' : 'Sign in'}
        </Button>
      </form>
    </AuthFrame>
  );
}

function LoadingFrame() {
  return (
    <AuthFrame kicker="Post Room" title="Sign in">
      <div role="status" aria-label="Loading" className="flex flex-col gap-5">
        <div className="flex flex-col gap-2">
          <Skeleton className="h-3 w-16" />
          <Skeleton className="h-10 w-full" />
        </div>
        <div className="flex flex-col gap-2">
          <Skeleton className="h-3 w-20" />
          <Skeleton className="h-10 w-full" />
        </div>
        <Skeleton className="h-11 w-full" />
      </div>
    </AuthFrame>
  );
}

export function SignInPage({ mode }: { mode: Mode }) {
  const config = useAuthConfig();
  const signupEnabled = config.data?.signup_enabled ?? true;
  const effectiveMode = mode === 'sign-up' && signupEnabled ? 'sign-up' : 'sign-in';

  return (
    <div className="flex min-h-dvh flex-col bg-paper">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <div className="grid flex-1 lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
        <BrandPanel />
        <main className="flex items-center justify-center px-4 py-10 sm:px-8 lg:py-14">
          {config.isLoading ? (
            <LoadingFrame />
          ) : config.data?.mode === 'dev' ? (
            <DevForm />
          ) : (
            <PasswordForm key={effectiveMode} mode={effectiveMode} signupEnabled={signupEnabled} />
          )}
        </main>
      </div>
    </div>
  );
}
