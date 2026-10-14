export type NavBadge = 'alerts' | 'agents';

export type NavItem = { to: string; label: string; badge?: NavBadge };

export type NavGroup = { id: string; label: string; items: NavItem[] };

/** `hasStaffing`: the HRMS group (timesheets, portal reconciliation, matching) and the HR workflows belong to the HR & recruiting pack. */
export function navGroups(isAdmin: boolean, hasStaffing = true): NavGroup[] {
  return [
    {
      id: 'work',
      label: 'Work',
      items: [
        { to: 'inbox', label: 'Inbox' },
        { to: 'agents', label: 'Agents', badge: 'agents' },
        { to: 'agent-team', label: 'Your team' },
        { to: 'metrics', label: 'Results' },
        { to: 'alerts', label: 'Alerts', badge: 'alerts' },
      ],
    },
    hasStaffing
      ? {
          id: 'hrms',
          label: 'HRMS',
          items: [
            { to: 'people', label: 'People' },
            { to: 'performance', label: 'Performance' },
            { to: 'timesheets', label: 'Timesheets' },
            { to: 'portal', label: 'Portal reconciliation' },
            { to: 'escalations', label: 'Escalations' },
            { to: 'matching', label: 'Matching' },
          ],
        }
      : {
          id: 'team',
          label: 'Team',
          items: [
            { to: 'people', label: 'People' },
            { to: 'performance', label: 'Performance' },
            { to: 'escalations', label: 'Escalations' },
          ],
        },
    {
      id: 'teach',
      label: 'Teach',
      items: [
        { to: 'labels', label: 'Labels' },
        { to: 'memory', label: 'Memory' },
        { to: 'automations', label: 'Automations' },
      ],
    },
    {
      id: 'settings',
      label: 'Settings',
      items: [
        { to: 'integrations', label: 'Integrations' },
        ...(hasStaffing ? [{ to: 'workflows', label: 'Workflows' }] : []),
        ...(isAdmin ? [{ to: 'packs', label: 'Packs' }] : []),
        { to: 'reviews', label: 'Reviews' },
        ...(isAdmin
          ? [
              { to: 'jobs', label: 'Jobs' },
              { to: 'usage', label: 'AI usage' },
              { to: 'ai-health', label: 'AI health' },
            ]
          : []),
      ],
    },
    {
      id: 'help',
      label: 'Help',
      items: [{ to: 'guide', label: 'Guide' }],
    },
  ];
}
