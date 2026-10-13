import { CountBadge } from '../../components/ui';
import { useOpenAlertCount } from './hooks';

export function AlertsBadge({ ws }: { ws: string }) {
  const count = useOpenAlertCount(ws);
  return <CountBadge count={count} label={`${count} open ${count === 1 ? 'alert' : 'alerts'}`} />;
}
