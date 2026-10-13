import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Button, Input, Spinner, Switch, TextLink, useToast } from '../../components/ui';
import { cn } from '../../lib/cn';
import { useCommand, useConfirmPlan, useTranscribe, useAssistantName } from './hooks';
import { PlanCardView } from './PlanCardView';
import { canSpeak, speak, stopSpeaking } from './speech';
import type { PlanState, Turn } from './types';
import { useVoiceInput } from './useVoiceInput';

const HOLD_MS = 350;
const EXAMPLES = ['What needs a reply?', 'Summarise the latest email from the client', 'Any approvals waiting for me?'];

const messageOf = (e: unknown) => (e instanceof Error ? e.message : 'Something went wrong');

function MicIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round" className="h-5 w-5">
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
    </svg>
  );
}

export function AssistantPanel({ ws, onClose }: { ws: string; onClose: () => void }) {
  const assistantName = useAssistantName(ws);
  const notify = useToast();
  const [turns, setTurns] = useState<Turn[]>([]);
  const [plans, setPlans] = useState<Record<string, PlanState>>({});
  const [text, setText] = useState('');
  const [readAloud, setReadAloud] = useState(false);
  const nextId = useRef(1);
  const listRef = useRef<HTMLOListElement>(null);
  const press = useRef<{ at: number; started: boolean; held: boolean } | undefined>(undefined);
  const command = useCommand(ws);
  const confirm = useConfirmPlan(ws);
  const transcribe = useTranscribe(ws);
  const speakable = canSpeak();

  useEffect(() => {
    const list = listRef.current;
    if (list) list.scrollTop = list.scrollHeight;
  }, [turns]);

  useEffect(() => {
    if (!readAloud) stopSpeaking();
    return stopSpeaking;
  }, [readAloud]);

  const setPlan = (id: string, state: PlanState) => setPlans((p) => ({ ...p, [id]: state }));

  const ask = (raw: string) => {
    const question = raw.trim();
    if (!question || command.isPending) return;
    setTurns((t) => [...t, { id: nextId.current++, role: 'user', text: question }]);
    setText('');
    command.mutate(question, {
      onSuccess: (res) => {
        setPlans((p) => ({ ...p, ...Object.fromEntries(res.plan.map((item) => [item.id, { status: 'pending' } as PlanState])) }));
        setTurns((t) => [
          ...t,
          { id: nextId.current++, role: 'assistant', text: res.answer, citations: res.citations, plan: res.plan, overBudget: res.over_budget },
        ]);
        if (readAloud) speak(res.answer);
      },
      onError: (e) =>
        setTurns((t) => [
          ...t,
          { id: nextId.current++, role: 'assistant', text: `I could not do that. ${messageOf(e)}`, citations: [], plan: [], overBudget: false },
        ]),
    });
  };

  const onConfirm = async (id: string) => {
    setPlan(id, { status: 'confirming' });
    try {
      const res = await confirm.mutateAsync(id);
      setPlan(id, { status: 'done', message: res.message, link: res.link });
      notify('success', res.message);
    } catch (e) {
      setPlan(id, { status: 'error', message: messageOf(e) });
      notify('error', messageOf(e));
    }
  };

  const voice = useVoiceInput(
    (blob, durationMs) =>
      transcribe.mutate(
        { blob, durationMs },
        {
          onSuccess: (res) => {
            if (res.text.trim()) ask(res.text);
            else notify('error', 'I did not catch that. Try again or type it.');
          },
          onError: (e) => notify('error', messageOf(e)),
        },
      ),
    (message) => notify('error', message),
  );
  const recording = voice.state === 'recording';

  const onMicPointerDown = () => {
    press.current = { at: Date.now(), started: !recording, held: false };
    if (!recording) void voice.start();
  };
  const onMicPointerUp = () => {
    const p = press.current;
    if (p?.started && Date.now() - p.at >= HOLD_MS) {
      p.held = true;
      voice.stop();
    }
  };
  const onMicClick = () => {
    const p = press.current;
    press.current = undefined;
    if (p?.started || p?.held) return;
    if (recording) voice.stop();
    else void voice.start();
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    ask(text);
  };

  return (
    <>
      <header className="flex items-center justify-between gap-2 border-b border-rule-strong px-4 py-3">
        <div className="min-w-0">
          <p className="kicker">Assistant</p>
          <h2 className="type-display text-xl leading-tight">Ask {assistantName}</h2>
        </div>
        <div className="flex items-center gap-3">
          {speakable && (
            <label className="flex items-center gap-2 text-[13px] text-ink-muted">
              Read aloud
              <Switch checked={readAloud} label="Read answers aloud" onChange={setReadAloud} />
            </label>
          )}
          <Button variant="ghost" size="sm" aria-label="Close assistant" onClick={onClose}>
            <span aria-hidden="true">×</span>
          </Button>
        </div>
      </header>

      <ol ref={listRef} aria-label="Conversation" aria-live="polite" className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-3">
        {turns.length === 0 && (
          <li className="flex flex-col gap-2 text-sm text-ink-muted">
            <p>Type or speak a request. I will answer from your mail, and ask before I do anything.</p>
            <ul className="flex flex-col gap-1">
              {EXAMPLES.map((example) => (
                <li key={example}>
                  <button type="button" onClick={() => ask(example)} className="rounded-xs text-left text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover">
                    {example}
                  </button>
                </li>
              ))}
            </ul>
          </li>
        )}
        {turns.map((turn) =>
          turn.role === 'user' ? (
            <li key={turn.id} className="flex justify-end">
              <p className="max-w-[85%] rounded-sm bg-ink px-3 py-2 text-sm text-paper">
                <span className="sr-only">You said: </span>
                {turn.text}
              </p>
            </li>
          ) : (
            <li key={turn.id} className="flex flex-col gap-2">
              <p className="max-w-[92%] rounded-sm border border-rule bg-raised px-3 py-2 text-sm text-ink">
                <span className="sr-only">{assistantName} said: </span>
                {turn.text}
              </p>
              {turn.citations.length > 0 && (
                <div className="flex flex-col gap-1 px-1">
                  <p className="kicker">Sources</p>
                  <ul className="flex flex-col gap-0.5 text-[13px]">
                    {turn.citations.map((c) => (
                      <li key={c.message_id} className="truncate">
                        <TextLink to={`/w/${ws}/inbox/${c.message_id}`}>{c.subject || '(no subject)'}</TextLink>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              {turn.plan.map((item) => (
                <PlanCardView
                  key={item.id}
                  ws={ws}
                  item={item}
                  state={plans[item.id] ?? { status: 'pending' }}
                  onConfirm={() => void onConfirm(item.id)}
                  onCancel={() => setPlan(item.id, { status: 'cancelled' })}
                />
              ))}
              {turn.overBudget && (
                <p className="px-1 text-[12px] text-ink-muted">Today's AI budget is used up. This ran because you asked.</p>
              )}
            </li>
          ),
        )}
        {(command.isPending || transcribe.isPending) && (
          <li className="flex items-center gap-2 text-sm text-ink-muted">
            <Spinner className="h-4 w-4" />
            {transcribe.isPending ? 'Listening back…' : 'Thinking…'}
          </li>
        )}
      </ol>

      <form onSubmit={onSubmit} className="flex flex-col gap-2 border-t border-rule-strong px-4 py-3">
        {recording && (
          <p role="status" className="text-[13px] text-accent">
            Listening… {voice.partial && <span className="text-ink-muted italic">{voice.partial}</span>}
          </p>
        )}
        <div className="flex gap-2">
          <Input
            value={text}
            onChange={(e) => setText(e.target.value)}
            placeholder={`Ask ${assistantName} anything about your mail`}
            aria-label={`Message to ${assistantName}`}
            maxLength={600}
            autoComplete="off"
          />
          <Button
            variant={recording ? 'primary' : 'secondary'}
            aria-pressed={recording}
            aria-label={voice.supported ? 'Talk to ' + assistantName : 'Voice input is not available in this browser'}
            title={voice.supported ? 'Hold to talk, or click to start and stop' : 'Voice input is not available in this browser'}
            disabled={!voice.supported || transcribe.isPending}
            className={cn('w-10 shrink-0 px-0', recording && 'motion-safe:animate-pulse')}
            onPointerDown={onMicPointerDown}
            onPointerUp={onMicPointerUp}
            onClick={onMicClick}
          >
            <MicIcon />
          </Button>
          <Button type="submit" disabled={!text.trim()} loading={command.isPending}>
            Send
          </Button>
        </div>
        <p className="text-[12px] text-ink-muted">Ctrl or Cmd + K opens this. Esc closes it.</p>
      </form>
    </>
  );
}
