import { Navigate } from 'react-router';
import { useWorkspace } from '../../app/useWorkspace';
import { Page, PageHeader } from '../../components/ui';
import { featureEnabled } from '../../lib/features';
import { FriendlyEmpty } from '../agents-hub/FriendlyEmpty';

/** Integration point for WP-D (metrics and evals). Replace the body with the dashboard; the route and flag stay. */
export function MetricsRoute() {
  const { ws } = useWorkspace();
  if (!featureEnabled('metrics')) return <Navigate to={`/w/${ws}/agent-team`} replace />;
  return (
    <Page>
      <PageHeader kicker="Results" title="How your agents are doing" />
      <FriendlyEmpty title="Scorecards are coming" text="Accuracy, approvals and cost per email will show up here." avatar="owl" />
    </Page>
  );
}
