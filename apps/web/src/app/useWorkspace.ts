import { useParams } from 'react-router';
import { useMe } from '../features/auth/hooks';

export function useWorkspace() {
  const { ws = '' } = useParams<{ ws: string }>();
  const me = useMe();
  const workspace = me.data?.workspaces.find((w) => w.id === ws);
  return {
    ws,
    workspace,
    isAdmin: workspace?.role === 'admin',
    hasStaffing: workspace?.packs?.includes('staffing') ?? true,
  };
}
