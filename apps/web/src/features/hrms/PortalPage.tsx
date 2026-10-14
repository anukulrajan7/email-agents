import { Badge, Button, Card, EmptyState, Page, PageHeader, SkeletonList, Table, Td, Th, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { humanize } from '../../lib/format';
import { ImportPanel } from '../team/ImportPanel';
import { useTeamAccess } from '../team/hooks';
import { useImportPortal, useMismatches, useReconcile } from './hooks';
import type { Mismatch } from './types';

const KIND: Record<string, string> = {
  hours_differ: 'Hours differ',
  missing_in_email: 'In the portal, no timesheet mail',
  missing_in_portal: 'Timesheet mail, not in the portal',
  submissions_missing_in_portal: 'Sent to a client, not in the portal',
  submissions_missing_in_email: 'In the portal, not sent by mail',
};

function detailText(m: Mismatch): string {
  const d = m.detail;
  if ('portal_hours' in d || 'email_hours' in d) return `Portal ${String(d.portal_hours ?? '–')} h · Mail ${String(d.email_hours ?? '–')} h`;
  if (Array.isArray(d.candidates)) return (d.candidates as string[]).join(', ');
  return '';
}

export function PortalPage() {
  const { ws } = useWorkspace();
  const access = useTeamAccess(ws);
  const mismatches = useMismatches(ws);
  const hours = useImportPortal(ws, 'hours');
  const subs = useImportPortal(ws, 'submissions');
  const reconcile = useReconcile(ws);
  const notify = useToast();
  const owner = access.data?.level === 'super_admin';
  const items = mismatches.data?.items ?? [];
  const noData = mismatches.data && mismatches.data.portal_hours_rows + mismatches.data.portal_submission_rows === 0;

  return (
    <Page className="gap-6">
      <PageHeader
        guide="portal-header"
        kicker="HRMS · Portal reconciliation"
        title="Portal reconciliation"
        description="Import hours and submissions exported from the client portal. They are compared exactly with the timesheet mails and the CVs you actually sent, and differences become alerts."
        actions={
          owner ? (
            <Button
              variant="secondary"
              loading={reconcile.isPending}
              onClick={() => reconcile.mutate(undefined, { onSuccess: (r) => notify('success', `Compared. ${r.alerts_raised} new alert${r.alerts_raised === 1 ? '' : 's'}.`) })}
            >
              Compare now
            </Button>
          ) : undefined
        }
      />
      <ErrorNote error={mismatches.error ?? reconcile.error} />
      {mismatches.isLoading && <SkeletonList rows={3} label="Loading mismatches" />}
      {owner && (
        <div className="grid gap-4 lg:grid-cols-2">
          <section aria-label="Import portal hours" className="flex flex-col gap-2">
            <p className="kicker">Hours per person per month</p>
            <ImportPanel importer={hours} label="Portal hours CSV" columns="email,month,hours" />
          </section>
          <section aria-label="Import portal submissions" className="flex flex-col gap-2">
            <p className="kicker">Submissions</p>
            <ImportPanel importer={subs} label="Portal submissions CSV" columns="email,date,candidate,client" />
          </section>
        </div>
      )}
      {mismatches.data &&
        (items.length === 0 ? (
          <Card>
            <EmptyState
              title={noData ? 'No portal data yet' : 'Everything matches'}
              description={
                noData
                  ? owner
                    ? 'Export last month from the client portal as CSV and paste it above, starting with the hours. People are matched by email, so add everyone in People first.'
                    : 'The super admin imports the portal exports. Differences for you will appear here.'
                  : 'The portal and your mail agree for everything imported so far.'
              }
            />
          </Card>
        ) : (
          <div className="rise" style={riseStyle(1)}>
            <Table>
              <thead>
                <tr>
                  <Th>Person</Th>
                  <Th>Month</Th>
                  <Th>Problem</Th>
                  <Th>Details</Th>
                  <Th>Status</Th>
                </tr>
              </thead>
              <tbody>
                {items.map((m) => (
                  <tr key={m.id}>
                    <Td className="font-semibold text-ink">{m.person_name}</Td>
                    <Td>{m.month}</Td>
                    <Td>{KIND[m.kind] ?? humanize(m.kind)}</Td>
                    <Td>{detailText(m)}</Td>
                    <Td>
                      <Badge tone={m.status === 'open' ? 'danger' : 'success'}>{humanize(m.status)}</Badge>
                    </Td>
                  </tr>
                ))}
              </tbody>
            </Table>
          </div>
        ))}
    </Page>
  );
}
