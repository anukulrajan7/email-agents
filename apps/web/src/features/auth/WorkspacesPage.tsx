import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { Badge, Button, Card, EmptyState, Input, Label, Page, Spinner } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { useLabelTemplates } from '../labels/hooks';
import type { LabelTemplate } from '../../api/types';
import { useCreateWorkspace, useLogout, useMe } from './hooks';

export function WorkspacesPage() {
  const me = useMe();
  const create = useCreateWorkspace();
  const navigate = useNavigate();
  const logout = useLogout();
  const [name, setName] = useState('');
  const [template, setTemplate] = useState<LabelTemplate>('hr');
  const templates = useLabelTemplates();

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    create.mutate({ name: trimmed, template }, {
      onSuccess: (w) => {
        setName('');
        navigate(`/w/${w.id}/inbox`);
      },
    });
  };

  return (
    <div className="flex h-dvh flex-col overflow-hidden">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <div className="min-h-0 flex-1">
      <Page>
        <header className="rise flex items-end justify-between gap-4 border-b border-rule-strong pb-6" style={riseStyle(0)}>
          <div>
            <p className="kicker">AI Inbox · Post Room</p>
            <h1 className="type-display mt-1 text-5xl tracking-[-0.025em]">Workspaces</h1>
            {me.data && (
              <p className="mt-2 text-sm text-ink-muted">
                Signed in as <span className="font-mono text-xs text-ink">{me.data.user.email}</span>
              </p>
            )}
          </div>
          <Button variant="secondary" size="sm" onClick={() => logout.mutate()}>
            Sign out
          </Button>
        </header>

        {me.isLoading && <Spinner />}
        <ErrorNote error={me.error} />

        {me.data && (
          <Card className="rise max-w-3xl p-0" style={riseStyle(1)}>
            {me.data.workspaces.length === 0 ? (
              <EmptyState title="No workspaces yet" description="A workspace holds one team's mail, labels and notes. Create your first one below." />
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

        <Card className="rise max-w-3xl bg-raised" style={riseStyle(2)}>
          <form onSubmit={submit} className="flex flex-col gap-3">
            <Label htmlFor="ws-name">Create workspace</Label>
            <div className="flex gap-2">
              <Input id="ws-name" required placeholder="Acme Inc" value={name} onChange={(e) => setName(e.target.value)} />
              <Button type="submit" loading={create.isPending}>
                Create
              </Button>
            </div>
            <fieldset className="mt-2 flex flex-col gap-2">
              <legend className="kicker">Start with a set of labels</legend>
              <p className="text-sm text-ink-muted">Labels are the categories the AI sorts your email into. You can change them later.</p>
              {templates.isLoading && <Spinner />}
              <ErrorNote error={templates.error} />
              <div className="grid gap-3 sm:grid-cols-3">
                {templates.data?.items.map((t) => (
                  <label
                    key={t.key}
                    className={cn(
                      'flex cursor-pointer flex-col gap-1 rounded-sm border p-3 text-sm has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent',
                      template === t.key ? 'border-ink bg-surface' : 'border-rule-strong hover:border-ink',
                    )}
                  >
                    <span className="flex items-center gap-2 font-semibold">
                      <input type="radio" name="label-template" value={t.key} checked={template === t.key} onChange={() => setTemplate(t.key)} className="accent-[var(--color-accent)]" />
                      {t.name}
                    </span>
                    <span className="text-ink-muted">{t.description}</span>
                    <span className="text-xs text-ink-muted">
                      {t.labels.length === 0 ? 'No labels — start from scratch' : t.labels.map((l) => l.name).join(', ')}
                    </span>
                  </label>
                ))}
              </div>
            </fieldset>
            <ErrorNote error={create.error} />
          </form>
        </Card>
      </Page>
      </div>
    </div>
  );
}
