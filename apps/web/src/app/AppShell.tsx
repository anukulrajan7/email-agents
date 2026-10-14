import { Suspense, useEffect, useId, useState } from 'react';
import { Link, NavLink, Outlet, useLocation, useNavigate } from 'react-router';
import { useLogout, useMe } from '../features/auth/hooks';
import { AlertsBadge } from '../features/alerts/AlertsBadge';
import { AgentsBadge } from '../features/agents/AgentsBadge';
import { OnboardingPrompt } from '../features/agents/OnboardingPrompt';
import { AskMeera } from '../features/assistant/AskMeera';
import { Button, Dialog, EmptyState, Skeleton, SkeletonList, Spinner } from '../components/ui';
import { cn } from '../lib/cn';
import { featureEnabled } from '../lib/features';
import { FirstRunTour } from '../features/help/FirstRunTour';
import { riseStyle } from '../lib/motion';
import { initialOf } from '../features/inbox/emailText';
import { navGroups, type NavBadge } from './nav';
import { useWorkspace } from './useWorkspace';

function Wordmark({ compact = false }: { compact?: boolean }) {
  return (
    <span className="flex flex-col">
      {!compact && <span className="kicker">The sorting office</span>}
      <span className={cn('type-display leading-none tracking-[-0.02em] text-ink', compact ? 'text-2xl' : 'mt-1 text-[2rem]')}>
        Post <span className="text-accent">Room</span>
      </span>
    </span>
  );
}

function Badge({ kind, ws }: { kind: NavBadge | undefined; ws: string }) {
  if (kind === 'alerts') return <AlertsBadge ws={ws} />;
  if (kind === 'agents') return <AgentsBadge ws={ws} />;
  return null;
}

function SideNav({ ws, isAdmin, hasStaffing }: { ws: string; isAdmin: boolean; hasStaffing: boolean }) {
  const uid = useId();
  return (
    <nav aria-label="Main" className="flex flex-col gap-6">
      {navGroups(isAdmin, hasStaffing, featureEnabled('metrics')).map((group) => (
        <div key={group.id} className="flex flex-col gap-1">
          <p id={`${uid}-${group.id}`} className="kicker px-3">
            {group.label}
          </p>
          <ul aria-labelledby={`${uid}-${group.id}`} className="flex flex-col gap-px">
            {group.items.map((item) => (
              <li key={item.to}>
                <NavLink
                  to={`/w/${ws}/${item.to}`}
                  className={({ isActive }) =>
                    cn(
                      'relative flex h-9 items-center gap-2 rounded-sm pr-2 pl-3 text-[15px] motion-safe:transition-colors',
                      'focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-accent',
                      'before:absolute before:top-1/2 before:left-0 before:h-5 before:w-[3px] before:-translate-y-1/2 before:rounded-xs',
                      isActive
                        ? 'bg-raised font-semibold text-ink shadow-[0_1px_0_var(--color-rule)] before:bg-accent'
                        : 'text-ink-muted before:bg-transparent hover:bg-ink/5 hover:text-ink',
                    )
                  }
                >
                  {item.label}
                  <Badge kind={item.badge} ws={ws} />
                </NavLink>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </nav>
  );
}

function AccountFooter({ workspaceName }: { workspaceName: string }) {
  const me = useMe();
  const logout = useLogout();
  const user = me.data?.user;
  const display = user?.name ?? user?.email ?? '';
  const many = (me.data?.workspaces.length ?? 0) > 1;

  return (
    <div className="flex flex-col gap-4 border-t border-rule-strong pt-4">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <p className="kicker">Workspace</p>
          <p className="type-display truncate text-lg leading-tight" title={workspaceName}>
            {workspaceName}
          </p>
        </div>
        <Link
          to="/workspaces"
          className="mt-4 shrink-0 rounded-xs text-[13px] font-semibold text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover"
        >
          {many ? 'Switch' : 'Manage'}
        </Link>
      </div>
      <div className="flex items-center gap-3">
        <span
          aria-hidden="true"
          className="flex h-9 w-9 shrink-0 items-center justify-center rounded-full border border-rule-strong bg-raised font-semibold text-ink"
        >
          {initialOf(display || '?')}
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-semibold text-ink" title={display}>
            {display}
          </p>
          {user?.name && (
            <p className="truncate font-mono text-[11px] text-ink-muted" title={user.email}>
              {user.email}
            </p>
          )}
        </div>
      </div>
      <Button variant="secondary" size="sm" loading={logout.isPending} onClick={() => logout.mutate()}>
        Sign out
      </Button>
    </div>
  );
}

function RouteFallback() {
  return (
    <div className="flex flex-col gap-8 px-4 pt-6 sm:px-6 lg:px-8">
      <div className="flex flex-col gap-2 border-b border-rule-strong pb-5">
        <Skeleton className="h-3 w-32" />
        <Skeleton className="h-9 w-64" />
      </div>
      <SkeletonList rows={3} label="Loading page" />
    </div>
  );
}

export function AppShell() {
  const { ws, workspace, isAdmin, hasStaffing } = useWorkspace();
  const me = useMe();
  const navigate = useNavigate();
  const location = useLocation();
  const [menuOpen, setMenuOpen] = useState(false);

  useEffect(() => {
    setMenuOpen(false);
  }, [location.pathname]);

  if (me.isLoading) {
    return (
      <div className="flex h-dvh flex-col items-center justify-center gap-4 text-ink-muted">
        <Wordmark compact />
        <Spinner />
      </div>
    );
  }

  if (!workspace) {
    return (
      <div className="flex min-h-dvh flex-col">
        <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
        <EmptyState
          title="Workspace not found"
          description="You are not a member of this workspace."
          action={<Button onClick={() => navigate('/workspaces')}>Choose a workspace</Button>}
        />
      </div>
    );
  }

  return (
    <div className="flex h-dvh flex-col overflow-hidden">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <header className="flex h-14 shrink-0 items-center justify-between gap-3 border-b border-rule-strong px-4 md:hidden">
        <Wordmark compact />
        <Button variant="secondary" size="sm" aria-expanded={menuOpen} aria-haspopup="dialog" onClick={() => setMenuOpen(true)}>
          Menu
        </Button>
      </header>
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <aside className="hidden w-60 shrink-0 flex-col overflow-y-auto border-r border-rule-strong px-3 pt-6 pb-5 md:flex">
          <div className="rise px-3" style={riseStyle(0)}>
            <Wordmark />
          </div>
          <div className="mt-8 flex-1">
            <SideNav ws={ws} isAdmin={isAdmin} hasStaffing={hasStaffing} />
          </div>
          <div className="mt-8 px-3">
            <AccountFooter workspaceName={workspace.name} />
          </div>
        </aside>
        <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
          <Suspense fallback={<RouteFallback />}>
            <Outlet />
          </Suspense>
        </main>
      </div>
      <OnboardingPrompt ws={ws} />
      <FirstRunTour />
      <AskMeera ws={ws} />
      {menuOpen && (
        <Dialog title="Menu" variant="drawer" side="left" onClose={() => setMenuOpen(false)} className="gap-6 bg-paper">
          <Button variant="ghost" size="sm" className="self-start" onClick={() => setMenuOpen(false)}>
            Close menu
          </Button>
          <SideNav ws={ws} isAdmin={isAdmin} hasStaffing={hasStaffing} />
          <AccountFooter workspaceName={workspace.name} />
        </Dialog>
      )}
    </div>
  );
}
