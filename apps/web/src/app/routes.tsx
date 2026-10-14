import { lazy } from 'react';
import { Navigate, Route, Routes } from 'react-router';
import { RequireAuth } from '../features/auth/RequireAuth';
import { SignInPage } from '../features/auth/SignInPage';
import { WorkspacesPage } from '../features/auth/WorkspacesPage';
import { InboxPage } from '../features/inbox/InboxPage';
import { ReviewsPage } from '../features/reviews/ReviewsPage';
import { WorkflowsPage } from '../features/workflows/WorkflowsPage';
import { JobsPage } from '../features/jobs/JobsPage';
import { LabelsPage } from '../features/labels/LabelsPage';
import { MemoryPage } from '../features/memory/MemoryPage';
import { ContactPage } from '../features/memory/ContactPage';
import { AlertsPage } from '../features/alerts/AlertsPage';
import { AgentsPage } from '../features/agents/AgentsPage';
import { PacksPage } from '../features/packs/PacksPage';
import { UsagePage } from '../features/usage/UsagePage';
import { PeoplePage } from '../features/team/PeoplePage';
import { PerformancePage } from '../features/team/PerformancePage';
import { EscalationsPage } from '../features/team/EscalationsPage';
import { MetricsRoute } from '../features/metrics/MetricsRoute';
import { AgentMetricsView } from '../features/metrics/AgentMetricsView';
import { AppShell } from './AppShell';

const IntegrationsPage = lazy(() => import('../features/sources/IntegrationsPage').then((m) => ({ default: m.IntegrationsPage })));
const MatchingPage = lazy(() => import('../features/matching/MatchingPage').then((m) => ({ default: m.MatchingPage })));
const TimesheetsPage = lazy(() => import('../features/hrms/TimesheetsPage').then((m) => ({ default: m.TimesheetsPage })));
const PortalPage = lazy(() => import('../features/hrms/PortalPage').then((m) => ({ default: m.PortalPage })));
const AutomationsPage = lazy(() => import('../features/automations/AutomationsPage').then((m) => ({ default: m.AutomationsPage })));
const CaseDetailPage = lazy(() => import('../features/agents/CaseDetailPage').then((m) => ({ default: m.CaseDetailPage })));

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/sign-in" element={<SignInPage mode="sign-in" />} />
      <Route path="/sign-up" element={<SignInPage mode="sign-up" />} />
      <Route element={<RequireAuth />}>
        <Route path="/workspaces" element={<WorkspacesPage />} />
        <Route path="/w/:ws" element={<AppShell />}>
          <Route index element={<Navigate to="inbox" replace />} />
          <Route path="inbox" element={<InboxPage />} />
          <Route path="inbox/:messageId" element={<InboxPage />} />
          <Route path="agents" element={<AgentsPage />} />
          <Route path="agents/:caseId" element={<CaseDetailPage />} />
          <Route path="metrics" element={<MetricsRoute />} />
          <Route path="metrics/:agentId" element={<AgentMetricsView />} />
          <Route path="matching" element={<MatchingPage />} />
          <Route path="people" element={<PeoplePage />} />
          <Route path="performance" element={<PerformancePage />} />
          <Route path="escalations" element={<EscalationsPage />} />
          <Route path="timesheets" element={<TimesheetsPage />} />
          <Route path="portal" element={<PortalPage />} />
          <Route path="labels" element={<LabelsPage />} />
          <Route path="reviews" element={<ReviewsPage />} />
          <Route path="alerts" element={<AlertsPage />} />
          <Route path="memory" element={<MemoryPage />} />
          <Route path="memory/contacts/:email" element={<ContactPage />} />
          <Route path="automations" element={<AutomationsPage />} />
          <Route path="workflows" element={<WorkflowsPage />} />
          <Route path="packs" element={<PacksPage />} />
          <Route path="integrations" element={<IntegrationsPage />} />
          <Route path="usage" element={<UsagePage />} />
          <Route path="jobs" element={<JobsPage />} />
        </Route>
      </Route>
      <Route path="*" element={<Navigate to="/workspaces" replace />} />
    </Routes>
  );
}
