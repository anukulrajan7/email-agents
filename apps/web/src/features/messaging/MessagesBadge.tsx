import { CountBadge } from '../../components/ui';
import { useUnreadCount } from './hooks';

export function MessagesBadge({ ws }: { ws: string }) {
  const count = useUnreadCount(ws);
  return <CountBadge count={count} label={`${count} unread ${count === 1 ? 'message' : 'messages'}`} />;
}
