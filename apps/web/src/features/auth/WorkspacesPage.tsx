import { useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { Button, Card, Input, Label, Skeleton, SkeletonList, Stamp } from '../../components/ui';
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
  const workspaces = me.data?.workspaces ?? [];
  const hasWorkspaces = workspaces.length > 0;

  const submit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) return;
    create.mutate(
      { name: trimmed, template },
      {
        onSuccess: (w) => {
          setName('');
          navigate(`/w/${w.id}/inbox`);
        },
      },
    );
  };

  return (
    <div className="flex h-dvh flex-col overflow-hidden">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <header className="flex shrink-0 items-center justify-between gap-4 border-b border-rule-strong bg-surface px-4 py-3 sm:px-8">
        <p className="type-display text-2xl leading-none tracking-[-0.02em]">
          Post <span className="text-accent">Room</span>
        </p>
        <div className="flex min-w-0 items-center gap-3">
          {me.data && (
            <span className="hidden truncate font-mono text-xs text-ink-muted sm:block" title={me.data.user.email}>
              {me.data.user.email}
            </span>
          )}
          <Button variant="secondary" size="sm" loading={logout.isPending} onClick={() => logout.mutate()}>
            Sign out
          </Button>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto flex w-full max-w-3xl flex-col gap-10 px-4 py-10 sm:px-8 lg:py-14">
          <div className="rise" style={riseStyle(0)}>
            <p className="kicker">{hasWorkspaces ? 'Your desks' : 'Welcome'}</p>
            <h1 className="type-display mt-2 text-[2.5rem] leading-[1.05] tracking-[-0.025em] sm:text-5xl">
              {hasWorkspaces ? 'Choose a workspace' : 'Set up your first workspace'}
            </h1>
            <p className="mt-3 max-w-[56ch] text-[15px] leading-relaxed text-ink-muted">
              A workspace holds one team&apos;s mail, labels and notes. {hasWorkspaces ? 'Open one to get to work, or start another.' : 'Name it and pick a starting set of labels.'}
            </p>
          </div>

          {me.isLoading && <SkeletonList rows={2} label="Loading workspaces" />}
          <ErrorNote error={me.error} />

          {hasWorkspaces && (
            <ul aria-label="Workspaces" className="rise flex flex-col gap-3" style={riseStyle(1)}>
              {workspaces.map((w) => (
                <li key={w.id}>
                  <Link
                    to={`/w/${w.id}/inbox`}
                    aria-label={`Open ${w.name}`}
                    className={cn(
                      'group letter flex items-center gap-4 rounded-xs px-5 py-4 motion-safe:transition-transform',
                      'hover:-translate-y-px hover:border-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
                    )}
                  >
                    <span aria-hidden="true" className="airmail h-10 w-1 shrink-0 rounded-full" />
                    <span className="min-w-0 flex-1">
                      <span className="type-display block truncate text-2xl leading-tight tracking-[-0.01em]">{w.name}</span>
                      <span className="mt-0.5 block text-sm text-ink-muted">{w.role === 'admin' ? 'You manage this workspace' : 'Member'}</span>
                    </span>
                    <Stamp name={w.role} color={w.role === 'admin' ? 'postmark' : 'rule'} />
                    <span aria-hidden="true" className="text-xl text-ink-muted group-hover:text-accent">
                      →
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          )}

          <Card className="rise bg-raised p-6 sm:p-8" style={riseStyle(2)}>
            <form onSubmit={submit} className="flex flex-col gap-6">
              <div>
                <h2 className="type-display text-2xl tracking-[-0.01em]">{hasWorkspaces ? 'Start a new workspace' : 'Create workspace'}</h2>
                <p className="mt-1 text-sm text-ink-muted">You can rename labels and invite people later.</p>
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="ws-name">Workspace name</Label>
                <div className="flex flex-col gap-2 sm:flex-row">
                  <Input id="ws-name" required placeholder="Acme Talent Desk" value={name} onChange={(e) => setName(e.target.value)} />
                  <Button type="submit" loading={create.isPending} disabled={!name.trim()} className="shrink-0">
                    Create
                  </Button>
                </div>
              </div>
              <fieldset className="flex flex-col gap-3">
                <legend className="kicker">Start with a set of labels</legend>
                <p className="-mt-1 text-sm text-ink-muted">Labels are the categories the AI sorts your email into.</p>
                {templates.isLoading && (
                  <div className="grid gap-3 sm:grid-cols-3">
                    {[0, 1, 2].map((i) => (
                      <Skeleton key={i} className="h-28" />
                    ))}
                  </div>
                )}
                <ErrorNote error={templates.error} />
                <div className="grid auto-rows-fr gap-3 sm:grid-cols-3">
                  {templates.data?.items.map((t) => (
                    <label
                      key={t.key}
                      className={cn(
                        'flex h-full cursor-pointer flex-col gap-1.5 rounded-sm border p-4 text-sm motion-safe:transition-colors',
                        'has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent',
                        template === t.key ? 'border-ink bg-surface shadow-[inset_0_3px_0_var(--color-accent)]' : 'border-rule-strong hover:border-ink',
                      )}
                    >
                      <span className="flex items-center gap-2 font-semibold">
                        <input
                          type="radio"
                          name="label-template"
                          value={t.key}
                          checked={template === t.key}
                          onChange={() => setTemplate(t.key)}
                          className="accent-[var(--color-accent)]"
                        />
                        {t.name}
                      </span>
                      <span className="text-ink-muted">{t.description}</span>
                      <span className="mt-auto pt-1 text-xs text-ink-muted">
                        {t.labels.length === 0 ? 'No labels — start from scratch' : t.labels.map((l) => l.name).join(' · ')}
                      </span>
                    </label>
                  ))}
                </div>
              </fieldset>
              <ErrorNote error={create.error} />
            </form>
          </Card>
        </div>
      </div>
    </div>
  );
}
