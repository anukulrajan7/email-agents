import { Link } from 'react-router';
import { Button } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useStartCase } from './hooks';
import type { StartCaseBody } from './types';

export function StartAgentButton({
  ws,
  target,
  label,
  ariaLabel,
}: {
  ws: string;
  target: StartCaseBody;
  label: string;
  ariaLabel?: string;
}) {
  const start = useStartCase(ws);

  if (start.data) {
    return (
      <p role="status" className="text-sm">
        <span className="text-success">Agent started. </span>
        <Link to={`/w/${ws}/agents/${start.data.id}`} className="font-semibold text-accent underline underline-offset-2 hover:text-accent-hover">
          View case
        </Link>
      </p>
    );
  }

  return (
    <div className="flex flex-col items-start gap-1">
      <Button size="sm" variant="secondary" loading={start.isPending} onClick={() => start.mutate(target)} aria-label={ariaLabel}>
        {label}
      </Button>
      <ErrorNote error={start.error} />
    </div>
  );
}
