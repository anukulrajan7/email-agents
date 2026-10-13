import { useEffect } from 'react';
import type { Insight, ProcessingStatus } from '../../api/types';
import { Badge } from '../../components/ui';
import { useUnderstandNow } from './hooks';

function Skeleton({ ws, messageId }: { ws: string; messageId: string }) {
  // Not read yet (older mail is read only when opened): ask for it now, once.
  const understand = useUnderstandNow(ws, messageId);
  const { mutate, isIdle } = understand;
  useEffect(() => {
    if (isIdle) mutate();
  }, [isIdle, mutate]);
  if (understand.isError) {
    return (
      <section aria-label="What they're asking" className="flex items-center justify-between gap-3 border border-rule bg-surface px-4 py-3">
        <p className="text-sm text-ink-muted">Couldn&apos;t read this email just now.</p>
        <button type="button" onClick={() => understand.mutate()} className="text-sm font-semibold text-accent underline underline-offset-2">
          Try again
        </button>
      </section>
    );
  }

  return (
    <section aria-label="What they're asking" aria-busy="true" className="flex flex-col gap-2 border border-rule bg-surface px-4 py-3">
      <h3 className="kicker">What they&apos;re asking</h3>
      <div aria-hidden="true" className="flex flex-col gap-2 motion-safe:animate-pulse">
        <div className="h-3 w-3/4 rounded-sm bg-rule" />
        <div className="h-3 w-1/2 rounded-sm bg-rule" />
      </div>
      <p className="text-sm text-ink-muted" role="status">
        Reading this email…
      </p>
    </section>
  );
}

export function InsightCard({
  ws,
  messageId,
  insight,
  status,
  replied = false,
}: {
  ws: string;
  messageId: string;
  insight: Insight | null | undefined;
  status: ProcessingStatus;
  replied?: boolean;
}) {
  if (insight === undefined) return null;
  if (insight === null) return status === 'failed' ? null : <Skeleton ws={ws} messageId={messageId} />;

  return (
    <section aria-label="What they're asking" className="flex flex-col gap-3 border border-rule bg-surface px-4 py-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h3 className="kicker">What they&apos;re asking</h3>
        <div className="flex flex-wrap items-center gap-1.5">
          {insight.deadline && <Badge tone="warning">Deadline: {insight.deadline}</Badge>}
          {replied ? (
            <Badge tone="success">Replied</Badge>
          ) : insight.needs_reply ? (
            <Badge tone="danger">Needs reply</Badge>
          ) : (
            <Badge>No reply needed</Badge>
          )}
        </div>
      </div>
      <p className="text-[15px] leading-snug font-semibold">{insight.summary}</p>
      {insight.asks.length > 0 && (
        <ul className="flex list-disc flex-col gap-1 pl-5 text-sm marker:text-ink-muted">
          {insight.asks.map((ask) => (
            <li key={ask}>{ask}</li>
          ))}
        </ul>
      )}
      {insight.missing_info.length > 0 && (
        <div className="border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
          <p className="font-semibold">Missing information</p>
          <ul className="mt-1 list-disc pl-5">
            {insight.missing_info.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
