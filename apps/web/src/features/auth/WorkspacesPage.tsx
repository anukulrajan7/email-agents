import { Link } from 'react-router';
import { Button, Card, SkeletonList, Stamp } from '../../components/ui';
import { riseStyle } from '../../lib/motion';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { GoalWizard } from '../onboarding/GoalWizard';
import { useLogout, useMe } from './hooks';

export function WorkspacesPage() {
  const me = useMe();
  const logout = useLogout();
  const workspaces = me.data?.workspaces ?? [];
  const hasWorkspaces = workspaces.length > 0;

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
              A workspace holds one team&apos;s mail, labels and notes. {hasWorkspaces ? 'Open one to get to work, or start another.' : 'Name it and pick what it is for.'}
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
            <GoalWizard />
          </Card>
        </div>
      </div>
    </div>
  );
}
