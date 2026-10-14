import { Navigate } from 'react-router';
import { useWorkspace } from '../../app/useWorkspace';
import { Page, PageHeader } from '../../components/ui';
import { featureEnabled } from '../../lib/features';
import { FriendlyEmpty } from '../agents-hub/FriendlyEmpty';

/** Integration point for WP-C ("Create an agent"). Replace the body with the builder; the route and flag stay. */
export function AgentBuilderRoute() {
  const { ws } = useWorkspace();
  if (!featureEnabled('agent-builder')) return <Navigate to={`/w/${ws}/agent-team`} replace />;
  return (
    <Page>
      <PageHeader kicker="Build" title="Create an agent" />
      <FriendlyEmpty title="The agent builder is on its way" text="Describe the job in plain words and an agent is drafted for you." avatar="robot" />
    </Page>
  );
}
