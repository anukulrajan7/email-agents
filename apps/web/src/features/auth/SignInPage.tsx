import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { setToken } from '../../api/client';
import { Button, Card, Input, Label } from '../../components/ui';
import { riseStyle } from '../../lib/motion';

export function SignInPage() {
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
    <div className="flex min-h-screen flex-col">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <div className="mx-auto grid w-full max-w-6xl flex-1 items-center gap-12 px-10 py-12 lg:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)]">
        <div className="rise" style={riseStyle(0)}>
          <p className="kicker">HR Inbox · Post Room</p>
          <h1 className="type-display mt-3 text-5xl leading-[1.05] tracking-[-0.025em] text-ink xl:text-6xl">
            Every HR email, sorted and answered{' '}
            <span className="text-accent">— by you.</span>
          </h1>
          <p className="mt-6 max-w-md text-[17px] text-ink-muted">
            Forward timesheets, leave requests and referrals. We sort them and draft the acknowledgement. Nothing is ever sent automatically.
          </p>
        </div>
        <Card className="rise w-full max-w-sm justify-self-center bg-raised p-8 lg:justify-self-end" style={riseStyle(2)}>
          <form onSubmit={submit} className="flex flex-col gap-5">
            <div>
              <p className="kicker">Dev sign-in</p>
              <h2 className="type-display mt-1 text-3xl tracking-[-0.02em]">Sign in</h2>
              <p className="mt-2 text-sm text-ink-muted">Enter any email to continue.</p>
            </div>
            <div className="flex flex-col gap-1">
              <Label htmlFor="email">Email</Label>
              <Input id="email" type="email" required autoFocus placeholder="admin@acme.test" value={email} onChange={(e) => setEmail(e.target.value)} />
            </div>
            <Button type="submit">Continue</Button>
            <p className="border-t border-rule pt-4 text-xs text-ink-muted">
              OIDC mode is configured server-side. This form only works when the API runs with{' '}
              <code className="font-mono">AUTH_MODE=dev</code>.
            </p>
          </form>
        </Card>
      </div>
    </div>
  );
}
