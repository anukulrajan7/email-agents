export type NavBadge = 'alerts' | 'agents';

export type NavItem = { to: string; label: string; badge?: NavBadge };

export type NavGroup = { id: string; label: string; items: NavItem[] };

/** `hasStaffing`: Matching and the HR workflows belong to the HR & recruiting pack. */
export function navGroups(isAdmin: boolean, hasStaffing = true): NavGroup[] {
  return [
    {
      id: 'work',
      label: 'Work',
      items: [
        { to: 'inbox', label: 'Inbox' },
        { to: 'agents', label: 'Agents', badge: 'agents' },
        { to: 'metrics', label: 'Results' },
        ...(hasStaffing ? [{ to: 'matching', label: 'Matching' }] : []),
        { to: 'alerts', label: 'Alerts', badge: 'alerts' },
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
            ]
          : []),
      ],
    },
  ];
}
