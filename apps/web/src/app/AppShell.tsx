import { NavLink, Outlet, useNavigate } from 'react-router';
import { useLogout, useMe } from '../features/auth/hooks';
import { Button, EmptyState, Label, Select, Spinner } from '../components/ui';
import { cn } from '../lib/cn';
import { riseStyle } from '../lib/motion';
import { useWorkspace } from './useWorkspace';

export function AppShell() {
  const { ws, workspace, isAdmin } = useWorkspace();
  const me = useMe();
  const navigate = useNavigate();
  const logout = useLogout();

  if (me.isLoading) {
    return (
      <div className="flex h-screen items-center justify-center text-ink-muted">
        <Spinner />
      </div>
    );
  }

  if (!workspace) {
    return (
      <EmptyState
        title="Workspace not found"
        description="You are not a member of this workspace."
        action={<Button onClick={() => navigate('/workspaces')}>Choose a workspace</Button>}
      />
    );
  }

  const items = [
    { to: 'inbox', label: 'Inbox' },
    { to: 'labels', label: 'Labels' },
    { to: 'reviews', label: 'Reviews' },
    { to: 'memory', label: 'Memory' },
    { to: 'workflows', label: 'Workflows' },
    { to: 'integrations', label: 'Integrations' },
    ...(isAdmin ? [{ to: 'jobs', label: 'Jobs' }] : []),
  ];

  return (
    <div className="flex h-dvh flex-col overflow-hidden">
      <div aria-hidden="true" className="airmail h-1.5 shrink-0" />
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <aside className="flex w-60 shrink-0 flex-col border-r border-rule-strong px-5 pt-5 pb-6">
          <div className="rise" style={riseStyle(0)}>
            <p className="kicker">AI Inbox</p>
            <p className="type-display text-3xl tracking-[-0.02em] text-ink">Post Room</p>
          </div>
          <div className="mt-6 flex flex-col gap-1">
            <Label htmlFor="ws-switcher">Workspace</Label>
            <Select
              id="ws-switcher"
              value={ws}
              onChange={(e) => (e.target.value === '__all' ? navigate('/workspaces') : navigate(`/w/${e.target.value}/inbox`))}
            >
              {me.data?.workspaces.map((w) => (
                <option key={w.id} value={w.id}>
                  {w.name}
                </option>
              ))}
              <option value="__all">Manage workspaces…</option>
            </Select>
          </div>
          <nav aria-label="Main" className="mt-6 flex flex-col gap-0.5">
            {items.map((item) => (
              <NavLink
                key={item.to}
                to={`/w/${ws}/${item.to}`}
                className={({ isActive }) =>
                  cn(
                    'relative rounded-sm py-2 pr-3 pl-4 text-[15px] motion-safe:transition-colors',
                    'focus-visible:outline-2 focus-visible:outline-offset-0 focus-visible:outline-accent',
                    'before:absolute before:top-1/2 before:left-0 before:h-5 before:w-[3px] before:-translate-y-1/2 before:rounded-xs',
                    isActive
                      ? 'bg-raised font-semibold text-ink before:bg-accent'
                      : 'text-ink-muted before:bg-transparent hover:bg-ink/5 hover:text-ink',
                  )
                }
              >
                {item.label}
              </NavLink>
            ))}
          </nav>
          <div className="mt-auto flex flex-col gap-3 border-t border-rule pt-4">
            <div>
              <p className="kicker">Signed in as</p>
              <p className="truncate font-mono text-xs text-ink" title={me.data?.user.email}>
                {me.data?.user.name ?? me.data?.user.email}
              </p>
            </div>
            <Button variant="secondary" size="sm" onClick={() => logout.mutate()}>
              Sign out
            </Button>
          </div>
        </aside>
        <main className="min-h-0 min-w-0 flex-1 overflow-hidden">
          <Outlet />
        </main>
      </div>
    </div>
  );
}
