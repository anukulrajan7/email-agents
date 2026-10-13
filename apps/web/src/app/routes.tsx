import { Navigate, Route, Routes } from 'react-router';
import { RequireAuth } from '../features/auth/RequireAuth';
import { SignInPage } from '../features/auth/SignInPage';
import { WorkspacesPage } from '../features/auth/WorkspacesPage';
import { InboxPage } from '../features/inbox/InboxPage';
import { IntegrationsPage } from '../features/sources/IntegrationsPage';
import { ReviewsPage } from '../features/reviews/ReviewsPage';
import { WorkflowsPage } from '../features/workflows/WorkflowsPage';
import { JobsPage } from '../features/jobs/JobsPage';
import { AppShell } from './AppShell';

export function AppRoutes() {
  return (
    <Routes>
      <Route path="/sign-in" element={<SignInPage />} />
      <Route element={<RequireAuth />}>
        <Route path="/workspaces" element={<WorkspacesPage />} />
        <Route path="/w/:ws" element={<AppShell />}>
          <Route index element={<Navigate to="inbox" replace />} />
          <Route path="inbox" element={<InboxPage />} />
          <Route path="inbox/:messageId" element={<InboxPage />} />
          <Route path="reviews" element={<ReviewsPage />} />
          <Route path="workflows" element={<WorkflowsPage />} />
          <Route path="integrations" element={<IntegrationsPage />} />
          <Route path="jobs" element={<JobsPage />} />
        </Route>
      </Route>
      <Route path="*" element={<Navigate to="/workspaces" replace />} />
    </Routes>
  );
}
