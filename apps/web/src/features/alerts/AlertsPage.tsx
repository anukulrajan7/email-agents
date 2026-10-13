import { useState } from 'react';
import { Card, EmptyState, PageHeader, Spinner, Page } from '../../components/ui';
import { Tabs, type TabItem } from '../../components/ui/Tabs';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { AlertCard } from './AlertCard';
import { AlertSettingsPanel } from './AlertSettingsPanel';
import { useAlerts, useUpdateAlert } from './hooks';

type AlertTab = 'open' | 'resolved' | 'settings';

const EMPTY = {
  open: { title: 'All clear', description: 'Nothing needs attention. When an important email goes unanswered or an approval is waiting, it shows up here and by email.' },
  resolved: { title: 'No resolved alerts yet', description: 'Alerts you resolve are kept here for reference.' },
};

function AlertList({ ws, status }: { ws: string; status: 'open' | 'resolved' }) {
  const alerts = useAlerts(ws, status);
  const update = useUpdateAlert(ws);
  const busyId = update.isPending ? update.variables?.id : undefined;

  return (
    <div className="flex flex-col gap-4">
      {alerts.isLoading && <Spinner />}
      <ErrorNote error={alerts.error ?? update.error} />
      {alerts.data &&
        (alerts.data.items.length === 0 ? (
          <Card>
            <EmptyState title={EMPTY[status].title} description={EMPTY[status].description} />
          </Card>
        ) : (
          <ul className="flex flex-col gap-3">
            {alerts.data.items.map((a) => (
              <AlertCard
                key={a.id}
                ws={ws}
                alert={a}
                busy={busyId === a.id}
                onResolve={() => update.mutate({ id: a.id, status: 'resolved' })}
                onReopen={() => update.mutate({ id: a.id, status: 'open' })}
                onSnooze={(hours) => update.mutate({ id: a.id, status: 'open', snooze_hours: hours })}
              />
            ))}
          </ul>
        ))}
    </div>
  );
}

export function AlertsPage() {
  const { ws, isAdmin } = useWorkspace();
  const [tab, setTab] = useState<AlertTab>('open');
  const open = useAlerts(ws, 'open');
  const openCount = open.data ? (open.data.open_count ?? open.data.items.length) : undefined;

  const tabs: TabItem<AlertTab>[] = [
    { id: 'open', label: 'Open', ...(openCount !== undefined ? { count: openCount } : {}) },
    { id: 'resolved', label: 'Resolved' },
    ...(isAdmin ? [{ id: 'settings' as const, label: 'Settings' }] : []),
  ];
  const current = tab === 'settings' && !isAdmin ? 'open' : tab;

  return (
    <Page className="gap-6">
      <PageHeader
        kicker="Nothing slips through"
        title="Alerts"
        description="The assistant tells you when an important email has gone unanswered or something is waiting on a person."
      />
      <div className="rise" style={riseStyle(1)}>
        <Tabs label="Alert views" tabs={tabs} value={current} onChange={setTab}>
          {current === 'settings' ? <AlertSettingsPanel ws={ws} /> : <AlertList ws={ws} status={current} />}
        </Tabs>
      </div>
    </Page>
  );
}
