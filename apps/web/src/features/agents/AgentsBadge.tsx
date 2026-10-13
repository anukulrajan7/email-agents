import { CountBadge } from '../../components/ui';
import { usePendingApprovalCount } from './hooks';

export function AgentsBadge({ ws }: { ws: string }) {
  const count = usePendingApprovalCount(ws);
  return <CountBadge count={count} label={`${count} ${count === 1 ? 'approval' : 'approvals'} waiting`} />;
}
