import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { useQueryClient } from '@tanstack/react-query';
import { clearToken } from '../../api/client';
import { Badge, Button, Card, EmptyState, Input, Label, Spinner } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { useCreateWorkspace, useMe } from './hooks';

export function WorkspacesPage() {
  const me = useMe();
  const create = useCreateWorkspace();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const [name, setName] = useState('');

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    create.mutate(trimmed, {
      onSuccess: (w) => {
        setName('');
        navigate(`/w/${w.id}/inbox`);
      },
    });
  };

  const signOut = () => {
    clearToken();
    qc.clear();
    navigate('/sign-in');
  };

  return (
    <div className="flex min-h-screen flex-col">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <div className="mx-auto flex w-full max-w-2xl flex-col gap-8 px-6 py-12">
        <header className="rise flex items-end justify-between gap-4 border-b border-rule-strong pb-6" style={riseStyle(0)}>
          <div>
            <p className="kicker">HR Inbox · Post Room</p>
            <h1 className="type-display mt-1 text-5xl tracking-[-0.025em]">Workspaces</h1>
            {me.data && (
              <p className="mt-2 text-sm text-ink-muted">
                Signed in as <span className="font-mono text-xs text-ink">{me.data.user.email}</span>
              </p>
            )}
          </div>
          <Button variant="secondary" size="sm" onClick={signOut}>
            Sign out
          </Button>
        </header>

        {me.isLoading && <Spinner />}
        <ErrorNote error={me.error} />

        {me.data && (
          <Card className="rise p-0" style={riseStyle(1)}>
            {me.data.workspaces.length === 0 ? (
              <EmptyState title="No workspaces yet" description="Create your first workspace below." />
            ) : (
              <ul className="divide-y divide-rule">
                {me.data.workspaces.map((w) => (
                  <li key={w.id} className="flex items-center justify-between gap-4 p-4 motion-safe:transition-colors hover:bg-ink/[0.03]">
                    <div>
                      <p className="type-display text-xl tracking-[-0.01em]">{w.name}</p>
                      <Badge className="mt-1">{w.role}</Badge>
                    </div>
                    <Button onClick={() => navigate(`/w/${w.id}/inbox`)} aria-label={`Open ${w.name}`}>
                      Open
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </Card>
        )}

        <Card className="rise bg-raised" style={riseStyle(2)}>
          <form onSubmit={submit} className="flex flex-col gap-3">
            <Label htmlFor="ws-name">Create workspace</Label>
            <div className="flex gap-2">
              <Input id="ws-name" required placeholder="Acme HR" value={name} onChange={(e) => setName(e.target.value)} />
              <Button type="submit" loading={create.isPending}>
                Create
              </Button>
            </div>
            <ErrorNote error={create.error} />
          </form>
        </Card>
      </div>
    </div>
  );
}
