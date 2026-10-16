import { Badge, RelativeTime, type Tone } from '../../components/ui';
import { ApprovalCard } from './ApprovalCard';
import { StepKindIcon, STEP_LABEL } from './icons';
import { formatCents } from './labels';
import type { Approval, Step } from './types';

const STEP_STATUS: Record<Step['status'], { label: string; tone: Tone } | undefined> = {
  ok: undefined,
  error: { label: 'Problem', tone: 'danger' },
  blocked: { label: 'Stopped by check', tone: 'warning' },
  waiting: { label: 'Waiting', tone: 'neutral' },
};

const pretty = (value: unknown) => JSON.stringify(value ?? null, null, 2);

function Raw({ step }: { step: Step }) {
  return (
    <details className="text-sm">
      <summary className="w-fit cursor-pointer rounded-xs text-ink-muted hover:text-ink">Technical details</summary>
      <div className="mt-2 flex flex-col gap-2">
        <div>
          <p className="kicker">Input</p>
          <pre className="max-h-64 overflow-auto rounded-xs border border-rule bg-raised p-2 font-mono text-xs break-words whitespace-pre-wrap">{pretty(step.input)}</pre>
        </div>
        <div>
          <p className="kicker">Output</p>
          <pre className="max-h-64 overflow-auto rounded-xs border border-rule bg-raised p-2 font-mono text-xs break-words whitespace-pre-wrap">{pretty(step.output)}</pre>
        </div>
      </div>
    </details>
  );
}

function DecidedApproval({ approval }: { approval: Approval }) {
  const approved = approval.status === 'approved';
  return (
    <p className="text-sm text-ink-muted">
      {approval.handoff?.line && <span className="block">{approval.handoff.line}</span>}
      <span className="font-semibold text-ink">{approved ? 'Approved' : 'Declined'}</span>
      {approval.decided_by ? ` by ${approval.decided_by}` : ''}
      {approval.decided_at && (
        <>
          {' '}
          <RelativeTime iso={approval.decided_at} />
        </>
      )}
    </p>
  );
}

export function Timeline({
  ws,
  steps,
  approvals,
  isAdmin,
}: {
  ws: string;
  steps: Step[];
  approvals: Approval[];
  isAdmin: boolean;
}) {
  const ordered = [...steps].sort((a, b) => a.seq - b.seq);
  const approvalSteps = ordered.filter((s) => s.kind === 'approval');
  const approvalFor = (step: Step) => approvals[approvalSteps.indexOf(step)];
  const leftover = approvals.slice(approvalSteps.length).filter((a) => a.status === 'pending');

  if (ordered.length === 0) return <p className="text-sm text-ink-muted">The agent has not taken any steps yet.</p>;

  return (
    <div className="flex flex-col gap-4">
      <ol aria-label="Steps taken" className="flex flex-col">
        {ordered.map((step, i) => {
          const approval = step.kind === 'approval' ? approvalFor(step) : undefined;
          const status = STEP_STATUS[step.status];
          return (
            <li key={step.seq} className="relative flex gap-4 pb-6 last:pb-0">
              {i < ordered.length - 1 && <span aria-hidden="true" className="absolute top-8 bottom-0 left-4 w-px -translate-x-1/2 bg-rule-strong" />}
              <span className="relative z-10 flex h-8 w-8 shrink-0 items-center justify-center rounded-full border border-rule-strong bg-surface text-ink-muted">
                <StepKindIcon kind={step.kind} />
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-2 pt-1">
                <p className="text-[15px] break-words">
                  <span className="sr-only">{STEP_LABEL[step.kind]}: </span>
                  {step.summary}
                </p>
                <p className="flex flex-wrap items-center gap-x-3 gap-y-1 font-mono text-xs text-ink-muted">
                  {status && <Badge tone={status.tone}>{status.label}</Badge>}
                  <span>{formatCents(step.cost_usd)}</span>
                  <RelativeTime iso={step.created_at} />
                </p>
                {isAdmin && <Raw step={step} />}
                {approval &&
                  (approval.status === 'pending' ? (
                    <ApprovalCard ws={ws} approval={approval} />
                  ) : (
                    <DecidedApproval approval={approval} />
                  ))}
              </div>
            </li>
          );
        })}
      </ol>
      {leftover.map((a) => (
        <ApprovalCard key={a.id} ws={ws} approval={a} />
      ))}
    </div>
  );
}
