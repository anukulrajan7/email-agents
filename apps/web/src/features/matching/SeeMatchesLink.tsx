import { Link } from 'react-router';
import { useRequirements } from './hooks';

export function SeeMatchesLink({ ws, messageId, category }: { ws: string; messageId: string; category: string }) {
  const isRequirement = category === 'client_requirement';
  const requirements = useRequirements(ws, isRequirement);
  const req = isRequirement ? requirements.data?.items.find((r) => r.message_id === messageId) : undefined;
  return (
    <Link
      to={`/w/${ws}/matching${req ? `?req=${req.id}` : ''}`}
      className="mt-3 inline-block text-sm font-semibold text-accent underline underline-offset-2 hover:text-accent-hover"
    >
      See matches →
    </Link>
  );
}
