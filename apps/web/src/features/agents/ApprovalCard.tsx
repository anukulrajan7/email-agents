import { useId, useState } from 'react';
import { Link } from 'react-router';
import { Button, Card, Label, RelativeTime, Textarea, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { AutomationSentence } from '../automations/Sentence';
import { useLabelMap } from '../labels/hooks';
import { AgentByline } from './AgentAvatar';
import { personName } from './agentLabels';
import { useDecideApproval } from './hooks';
import type { Approval, SuggestedAutomation } from './types';

const HAS_GAPS = /\[NEEDS:/i;

const WANTS: Record<Approval['action'], string> = {
  send_email: ' wants to send a reply',
  submit_candidate: ' wants to submit candidates',
  create_automation: ' suggests an automation',
};

function AutomationPreview({ ws, automation, reason }: { ws: string; automation: SuggestedAutomation; reason: string }) {
  const labels = useLabelMap(ws);
  return (
    <div className="flex flex-col gap-2 rounded-sm border border-rule bg-raised p-3">
      <p className="font-semibold text-ink">{automation.name}</p>
      <AutomationSentence conditions={automation.conditions} actions={automation.actions} labelName={(k) => labels.get(k)?.name ?? k} />
      {reason && <p className="text-sm text-ink-muted">Why: {reason}</p>}
      <p className="text-sm text-ink-muted">Approving creates this automation and turns it on. Only an admin can approve it.</p>
    </div>
  );
}

export function ApprovalCard({ ws, approval }: { ws: string; approval: Approval }) {
  const uid = useId();
  const notify = useToast();
  const decide = useDecideApproval(ws);
  const [body, setBody] = useState(approval.body);
  const [editing, setEditing] = useState(false);
  const [declining, setDeclining] = useState(false);
  const [note, setNote] = useState('');
  const edited = body !== approval.body;
  const busy = decide.isPending;

  const isAutomation = approval.action === 'create_automation' && approval.automation;

  const approve = async () => {
    try {
      await decide.mutateAsync({ id: approval.id, decision: 'approve', ...(edited && !isAutomation ? { edited_body: body } : {}) });
      notify('success', isAutomation ? 'Automation created and turned on.' : `Approved and sent${approval.to ? ` to ${approval.to}` : ''}.`);
    } catch {
      notify('error', 'Could not send. Nothing was sent.');
    }
  };

  const decline = async () => {
    const trimmed = note.trim();
    try {
      await decide.mutateAsync({ id: approval.id, decision: 'decline', ...(trimmed ? { note: trimmed } : {}) });
      notify('success', 'Declined. Nothing was sent.');
    } catch {
      notify('error', 'Could not save your decision. Try again.');
    }
  };

  return (
    <Card className="flex flex-col gap-4 p-5" aria-label={approval.summary} role="group">
        <div className="flex flex-col gap-1">
          {approval.agent && (
            <AgentByline agent={approval.agent}>
              {WANTS[approval.action]}
              {approval.working_for ? ` for ${personName(approval.working_for)}` : ''}
            </AgentByline>
          )}
          {approval.handoff?.line && <p className="text-sm text-ink-muted">{approval.handoff.line}</p>}
          <h3 className="type-display text-xl tracking-[-0.01em] break-words">{approval.summary}</h3>
          <p className="text-sm text-ink-muted">
            <Link to={`/w/${ws}/agents/${approval.case_id}`} className="underline decoration-current/40 underline-offset-[3px] hover:text-ink">
              {approval.case_title}
            </Link>{' '}
            · <RelativeTime iso={approval.created_at} />
          </p>
        </div>

        {(approval.to || approval.subject) && (
          <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-sm">
            {approval.to && (
              <>
                <dt className="kicker self-center">To</dt>
                <dd className="font-mono text-xs break-all">{approval.to}</dd>
              </>
            )}
            {approval.subject && (
              <>
                <dt className="kicker self-center">Subject</dt>
                <dd className="font-semibold break-words">{approval.subject}</dd>
              </>
            )}
          </dl>
        )}

        {approval.action === 'submit_candidate' && approval.candidates.length > 0 && (
          <ul aria-label="Candidates in this submission" className="flex flex-wrap gap-2">
            {approval.candidates.map((c) => (
              <li key={c.id} className="inline-flex items-center gap-2 rounded-xs border border-rule-strong bg-raised px-2 py-1 text-sm">
                <span className="font-semibold">{c.name}</span>
                <span className="font-mono text-xs text-ink-muted" aria-label={`score ${c.score} of 100`}>
                  {c.score}
                </span>
              </li>
            ))}
          </ul>
        )}

        {isAutomation && approval.automation ? (
          <AutomationPreview ws={ws} automation={approval.automation} reason={approval.body} />
        ) : (
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-body`}>Email body</Label>
          <Textarea id={`${uid}-body`} rows={8} value={body} onChange={(e) => setBody(e.target.value)} readOnly={!editing} disabled={busy} className="text-sm" />
          {(approval.unanswered?.length ?? 0) > 0 && (
            <p role="note" className="text-sm text-review-ink">
              Couldn't confirm: {approval.unanswered?.join(', ')}
            </p>
          )}
          {HAS_GAPS.test(body) && (
            <p className="text-sm text-review-ink">This draft still has gaps marked [NEEDS: …]. Fill them in before sending.</p>
          )}
          {edited && (
            <button
              type="button"
              onClick={() => setBody(approval.body)}
              className="self-start text-sm text-accent underline decoration-current/40 underline-offset-[3px] hover:text-accent-hover"
            >
              Undo my edits
            </button>
          )}
        </div>
        )}

        {declining && (
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-note`}>Note for the agent (optional)</Label>
            <Textarea id={`${uid}-note`} rows={2} value={note} onChange={(e) => setNote(e.target.value)} disabled={busy} className="text-sm" />
          </div>
        )}

        <ErrorNote error={decide.error} />

        <div className="flex flex-wrap items-center gap-2">
          {declining ? (
            <>
              <Button variant="danger" onClick={decline} loading={busy}>
                Confirm not now
              </Button>
              <Button variant="ghost" onClick={() => setDeclining(false)} disabled={busy}>
                Back
              </Button>
            </>
          ) : (
            <>
              <Button onClick={approve} loading={busy}>
                {isAutomation ? 'Create automation' : 'Send'}
              </Button>
              {!isAutomation && (
                <Button variant="secondary" onClick={() => setEditing(true)} disabled={busy || editing}>
                  Edit
                </Button>
              )}
              <Button variant="ghost" onClick={() => setDeclining(true)} disabled={busy}>
                Not now
              </Button>
            </>
          )}
        </div>
    </Card>
  );
}
