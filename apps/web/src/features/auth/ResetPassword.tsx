import { useState, type FormEvent } from 'react';
import { useMutation } from '@tanstack/react-query';
import { api, ApiError } from '../../api/client';
import { Button, Input, Label } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';

const MIN_PASSWORD = 10;
const linkClass = 'font-semibold text-accent underline underline-offset-2 hover:text-accent-hover';

type ConfirmBody = { email: string; code: string; new_password: string };

function resetErrorMessage(error: unknown): Error | undefined {
  if (!error) return undefined;
  if (error instanceof ApiError) {
    if (error.status === 400) return new Error('That code is invalid or expired.');
    if (error.status === 429) return new Error('Too many attempts. Try again in a few minutes.');
  }
  return error instanceof Error ? error : new Error('Something went wrong');
}

export function ResetPassword({ initialEmail, onDone }: { initialEmail: string; onDone: (notice?: string) => void }) {
  const [email, setEmail] = useState(initialEmail);
  const [step, setStep] = useState<'request' | 'confirm'>('request');
  const [code, setCode] = useState('');
  const [password, setPassword] = useState('');
  const request = useMutation({
    mutationFn: (address: string) => api<{ status: 'sent' }>('/auth/password-reset/request', { method: 'POST', body: { email: address } }),
    onSuccess: () => setStep('confirm'),
  });
  const confirm = useMutation({
    mutationFn: (body: ConfirmBody) => api<{ status: 'password_reset' }>('/auth/password-reset/confirm', { method: 'POST', body }),
    onSuccess: () => onDone('Password changed. Sign in with your new password.'),
  });
  const tooShort = password.length < MIN_PASSWORD;

  const submitRequest = (e: FormEvent) => {
    e.preventDefault();
    if (request.isPending || !email.trim()) return;
    request.mutate(email.trim());
  };

  const submitConfirm = (e: FormEvent) => {
    e.preventDefault();
    if (confirm.isPending || code.length !== 6 || tooShort) return;
    confirm.mutate({ email: email.trim(), code, new_password: password });
  };

  if (step === 'request') {
    return (
      <form onSubmit={submitRequest} className="flex flex-col gap-5">
        <div>
          <p className="kicker">Forgot password</p>
          <h2 className="type-display mt-1 text-3xl tracking-[-0.02em]">Reset your password</h2>
          <p className="mt-2 text-sm text-ink-muted">We'll email you a 6-digit code if this address has an account.</p>
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor="reset-email">Email</Label>
          <Input id="reset-email" type="email" required autoFocus autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} />
        </div>
        <ErrorNote error={resetErrorMessage(request.error)} />
        <Button type="submit" loading={request.isPending}>
          Send code
        </Button>
        <button type="button" className={`${linkClass} self-start text-sm`} onClick={() => onDone()}>
          Back to sign in
        </button>
      </form>
    );
  }

  return (
    <form onSubmit={submitConfirm} className="flex flex-col gap-5">
      <div>
        <p className="kicker">Forgot password</p>
        <h2 className="type-display mt-1 text-3xl tracking-[-0.02em]">Choose a new password</h2>
        <p className="mt-2 text-sm text-ink-muted">If {email.trim()} has an account, a code is on its way. It expires in 10 minutes.</p>
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor="reset-code">Code</Label>
        <Input
          id="reset-code"
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
      <div className="flex flex-col gap-1">
        <Label htmlFor="reset-password">New password</Label>
        <Input
          id="reset-password"
          type="password"
          required
          autoComplete="new-password"
          maxLength={128}
          aria-describedby="reset-password-hint"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
        <p id="reset-password-hint" className="text-xs text-ink-muted">
          At least {MIN_PASSWORD} characters. Every device is signed out.
        </p>
      </div>
      <ErrorNote error={resetErrorMessage(confirm.error)} />
      <Button type="submit" loading={confirm.isPending} disabled={code.length !== 6 || tooShort}>
        Set new password
      </Button>
      <button type="button" className={`${linkClass} self-start text-sm`} onClick={() => onDone()}>
        Back to sign in
      </button>
    </form>
  );
}
