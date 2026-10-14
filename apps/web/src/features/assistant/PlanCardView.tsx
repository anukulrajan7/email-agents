import { Button, TextLink } from '../../components/ui';
import { ACTION_LABEL, linkTarget } from './labels';
import type { PlanItem, PlanPreview, PlanState } from './types';

function MessagePreview({ preview }: { preview: PlanPreview }) {
  return (
    <dl className="flex flex-col gap-1 rounded-xs border border-rule bg-paper px-2 py-1.5 text-[13px]">
      <div className="flex gap-2">
        <dt className="kicker shrink-0">To</dt>
        <dd className="min-w-0 break-words text-ink">
          {preview.to}
          {preview.address && <span className="text-ink-muted"> ({preview.address})</span>}
        </dd>
      </div>
      {preview.source && (
        <div className="flex gap-2">
          <dt className="kicker shrink-0">Address from</dt>
          <dd className="text-ink-muted">{preview.source}</dd>
        </div>
      )}
      <div className="flex gap-2">
        <dt className="kicker shrink-0">Who sees it</dt>
        <dd className={preview.outside ? 'font-semibold text-ink' : 'text-ink-muted'}>{preview.audience}</dd>
      </div>
      <div className="flex gap-2">
        <dt className="kicker shrink-0">Subject</dt>
        <dd className="min-w-0 break-words text-ink">{preview.subject}</dd>
      </div>
      <div className="mt-1">
        <dt className="sr-only">Message</dt>
        <dd className="break-words whitespace-pre-wrap text-ink">{preview.body}</dd>
      </div>
    </dl>
  );
}

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
      {item.params.preview && <MessagePreview preview={item.params.preview} />}
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
