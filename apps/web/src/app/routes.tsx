import { Navigate, Route, Routes } from 'react-router';
import { RequireAuth } from '../features/auth/RequireAuth';
import { SignInPage } from '../features/auth/SignInPage';
import { WorkspacesPage } from '../features/auth/WorkspacesPage';
import { InboxPage } from '../features/inbox/InboxPage';
import { IntegrationsPage } from '../features/sources/IntegrationsPage';
import { ReviewsPage } from '../features/reviews/ReviewsPage';
import { WorkflowsPage } from '../features/workflows/WorkflowsPage';
import { JobsPage } from '../features/jobs/JobsPage';
import { LabelsPage } from '../features/labels/LabelsPage';
import { MatchingPage } from '../features/matching/MatchingPage';
import { MemoryPage } from '../features/memory/MemoryPage';
import { ContactPage } from '../features/memory/ContactPage';
import { AlertsPage } from '../features/alerts/AlertsPage';
import { AppShell } from './AppShell';

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
          <Route path="matching" element={<MatchingPage />} />
          <Route path="labels" element={<LabelsPage />} />
          <Route path="reviews" element={<ReviewsPage />} />
          <Route path="alerts" element={<AlertsPage />} />
          <Route path="memory" element={<MemoryPage />} />
          <Route path="memory/contacts/:email" element={<ContactPage />} />
          <Route path="workflows" element={<WorkflowsPage />} />
          <Route path="integrations" element={<IntegrationsPage />} />
          <Route path="jobs" element={<JobsPage />} />
        </Route>
      </Route>
      <Route path="*" element={<Navigate to="/workspaces" replace />} />
    </Routes>
  );
}
