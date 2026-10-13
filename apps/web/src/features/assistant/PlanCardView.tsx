import { Button, TextLink } from '../../components/ui';
import { ACTION_LABEL, linkTarget } from './labels';
import type { PlanItem, PlanState } from './types';

export function PlanCardView({
  ws,
  item,
  state,
  onConfirm,
  onCancel,
}: {
  ws: string;
  item: PlanItem;
  state: PlanState;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const open = state.status === 'pending' || state.status === 'confirming' || state.status === 'error';
  return (
    <article aria-label={`${ACTION_LABEL[item.action]}: ${item.summary}`} className="flex flex-col gap-2 rounded-sm border border-rule-strong border-l-[3px] border-l-accent bg-raised px-3 py-2">
      <p className="kicker">{ACTION_LABEL[item.action]}</p>
      <p className="text-sm text-ink">{item.summary}</p>
      {open && (
        <div className="flex gap-2">
          <Button size="sm" loading={state.status === 'confirming'} aria-label={`Confirm: ${item.summary}`} onClick={onConfirm}>
            Confirm
          </Button>
          <Button size="sm" variant="ghost" disabled={state.status === 'confirming'} aria-label={`Cancel: ${item.summary}`} onClick={onCancel}>
            Cancel
          </Button>
        </div>
      )}
      {state.status === 'error' && (
        <p role="alert" className="text-[13px] text-danger">
          {state.message}
        </p>
      )}
      {state.status === 'done' && (
        <p className="text-[13px] text-success">
          <span aria-hidden="true">✓ </span>
          {state.message}
          {state.link && (
            <>
              {' '}
              <TextLink to={linkTarget(ws, state.link).to}>{linkTarget(ws, state.link).label}</TextLink>
            </>
          )}
        </p>
      )}
      {state.status === 'expired' && <p className="text-[13px] text-ink-muted">This card expired. Ask again if you still want it.</p>}
      {state.status === 'cancelled' && <p className="text-[13px] text-ink-muted">Cancelled. Nothing was done.</p>}
    </article>
  );
}
