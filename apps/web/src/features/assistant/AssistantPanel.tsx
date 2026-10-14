import { Fragment, useLayoutEffect, useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { useLocation } from 'react-router';
import { ApiError } from '../../api/client';
import { Button, Input, Spinner, Switch, TextLink, useToast } from '../../components/ui';
import { cn } from '../../lib/cn';
import { useCommand, useConfirmPlan, useConversation, useTranscribe } from './hooks';
import { ASSISTANT_NAME as assistantName } from './labels';
import { dayKey, dayLabel, linkTarget } from './labels';
import { readPageContext } from './pageContext';
import { PlanCardView } from './PlanCardView';
import { canSpeak, speak, stopSpeaking, unlockSpeech } from './speech';
import type { HistoryMessage, PlanItem, PlanState, StoredPlanItem, Turn } from './types';
import { useVoiceInput } from './useVoiceInput';

const HOLD_MS = 350;
const EXAMPLES = ['What needs a reply?', 'Summarise the latest email from the client', 'Any approvals waiting for me?'];
const EMAIL_EXAMPLES = ['What is this email about?', 'Read the attachments for me', 'Draft a reply to this email'];

const messageOf = (e: unknown) => (e instanceof Error ? e.message : 'Something went wrong');

const failureText = (e: unknown) =>
  e instanceof ApiError && e.code === 'ai_unavailable'
    ? "I can't reach the AI service right now, so I could not answer. Your question is back in the box: try again in a moment."
    : `I could not do that. ${messageOf(e)}`;

const fromHistory = (m: HistoryMessage): Turn =>
  m.role === 'user'
    ? { id: `h${m.id}`, role: 'user', text: m.text, at: m.created_at }
    : { id: `h${m.id}`, role: 'assistant', text: m.text, at: m.created_at, citations: m.citations, plan: m.plan, overBudget: false, ...(m.links ? { links: m.links } : {}) };

function initialPlanState(item: PlanItem | StoredPlanItem): PlanState {
  if (!('state' in item)) return { status: 'pending' };
  if (item.state === 'done') return { status: 'done', message: 'Confirmed earlier.', link: null };
  return item.state === 'expired' ? { status: 'expired' } : { status: 'pending' };
}

function MicIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round" className="h-5 w-5">
      <rect x="9" y="3" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
    </svg>
  );
}

export function AssistantPanel({ ws, onClose }: { ws: string; onClose: () => void }) {
  const notify = useToast();
  const location = useLocation();
  const viewing = readPageContext(location.pathname, location.search);
  const [live, setLive] = useState<Turn[]>([]);
  const [plans, setPlans] = useState<Record<string, PlanState>>({});
  const [text, setText] = useState('');
  const [readAloud, setReadAloud] = useState(false);
  const nextId = useRef(1);
  const scrollRef = useRef<HTMLDivElement>(null);
  const keepPlace = useRef<number | undefined>(undefined);
  const press = useRef<{ at: number; started: boolean; held: boolean } | undefined>(undefined);
  const command = useCommand(ws);
  const confirm = useConfirmPlan(ws);
  const transcribe = useTranscribe(ws);
  const conversation = useConversation(ws);
  const speakable = canSpeak();

  const past = useMemo(
    () =>
      (conversation.data?.pages ?? [])
        .slice()
        .reverse()
        .flatMap((page) => page.items.map(fromHistory)),
    [conversation.data],
  );

  useLayoutEffect(() => {
    const list = scrollRef.current;
    if (!list) return;
    if (keepPlace.current !== undefined) {
      list.scrollTop += list.scrollHeight - keepPlace.current;
      keepPlace.current = undefined;
    } else {
      list.scrollTop = list.scrollHeight;
    }
  }, [past.length, live.length, command.isPending]);

  useEffect(() => {
    if (!readAloud) stopSpeaking();
    return stopSpeaking;
  }, [readAloud]);

  const setPlan = (id: string, state: PlanState) => setPlans((p) => ({ ...p, [id]: state }));
  const nowIso = () => new Date().toISOString();

  const loadOlder = () => {
    const list = scrollRef.current;
    if (!list || !conversation.hasNextPage || conversation.isFetchingNextPage) return;
    keepPlace.current = list.scrollHeight;
    void conversation.fetchNextPage();
  };

  const ask = (raw: string) => {
    const question = raw.trim();
    if (!question || command.isPending) return;
    setLive((t) => [...t, { id: `l${nextId.current++}`, role: 'user', text: question, at: nowIso() }]);
    setText('');
    command.mutate(
      { text: question, context: viewing.context },
      {
        onSuccess: (res) => {
          setPlans((p) => ({ ...p, ...Object.fromEntries(res.plan.map((item) => [item.id, { status: 'pending' } as PlanState])) }));
          setLive((t) => [
            ...t,
            { id: `l${nextId.current++}`, role: 'assistant', text: res.answer, at: nowIso(), citations: res.citations, plan: res.plan, overBudget: res.over_budget },
          ]);
          if (readAloud) speak(res.answer);
        },
        onError: (e) => {
          setText((current) => current || question);
          setLive((t) => [
            ...t,
            { id: `l${nextId.current++}`, role: 'assistant', text: failureText(e), at: nowIso(), citations: [], plan: [], overBudget: false, failed: true },
          ]);
        },
      },
    );
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

  const renderTurn = (turn: Turn, previous: Turn | undefined) => (
    <Fragment key={turn.id}>
      {(!previous || dayKey(turn.at) !== dayKey(previous.at)) && <li className="kicker py-1 text-center">{dayLabel(turn.at)}</li>}
      {turn.role === 'user' ? (
        <li className="flex justify-end">
          <p className="max-w-[85%] rounded-sm bg-ink px-3 py-2 text-sm text-paper">
            <span className="sr-only">You said: </span>
            {turn.text}
          </p>
        </li>
      ) : (
        <li className="flex flex-col gap-2">
          <p
            role={turn.failed ? 'alert' : undefined}
            className={cn('max-w-[92%] rounded-sm border bg-raised px-3 py-2 text-sm text-ink', turn.failed ? 'border-danger' : 'border-rule')}
          >
            <span className="sr-only">{assistantName} said: </span>
            {turn.text}
          </p>
          {turn.links && turn.links.length > 0 && (
            <ul className="flex flex-col gap-0.5 px-1 text-[13px]">
              {turn.links.map((link) => (
                <li key={`${link.kind}:${link.id ?? ''}`}>
                  <TextLink to={linkTarget(ws, link).to}>{link.label ?? linkTarget(ws, link).label}</TextLink>
                </li>
              ))}
            </ul>
          )}
          {turn.citations.length > 0 && (
            <div className="flex flex-col gap-1 px-1">
              <p className="kicker">Sources</p>
              <ul className="flex flex-col gap-0.5 text-[13px]">
                {turn.citations.map((c) => (
                  <li key={`${c.message_id}:${c.attachment_id ?? ''}`} className="truncate">
                    {c.attachment_name ? (
                      <>
                        <TextLink to={`/w/${ws}/inbox/${c.message_id}`} aria-label={`${c.attachment_name}, attached to ${c.subject || 'an email'}`}>
                          {c.attachment_name}
                        </TextLink>
                        <span className="text-ink-muted"> · attached to {c.subject || '(no subject)'}</span>
                      </>
                    ) : (
                      <TextLink to={`/w/${ws}/inbox/${c.message_id}`}>{c.subject || '(no subject)'}</TextLink>
                    )}
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
              state={plans[item.id] ?? initialPlanState(item)}
              onConfirm={() => void onConfirm(item.id)}
              onCancel={() => setPlan(item.id, { status: 'cancelled' })}
            />
          ))}
          {turn.overBudget && <p className="px-1 text-[12px] text-ink-muted">Today's AI budget is used up. This ran because you asked.</p>}
        </li>
      )}
    </Fragment>
  );

  const empty = !conversation.isPending && past.length === 0 && live.length === 0;
  const examples = viewing.label === 'this email' ? EMAIL_EXAMPLES : EXAMPLES;

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
              <Switch
                checked={readAloud}
                label="Read answers aloud"
                onChange={(on) => {
                  if (on) unlockSpeech();
                  setReadAloud(on);
                }}
              />
            </label>
          )}
          <Button variant="ghost" size="sm" aria-label="Close assistant" onClick={onClose}>
            <span aria-hidden="true">×</span>
          </Button>
        </div>
      </header>
      {viewing.label && (
        <p className="flex items-center gap-2 border-b border-rule bg-accent-tint/50 px-4 py-1.5 text-[12px] text-ink">
          <span aria-hidden="true" className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />
          <span>
            Looking at <strong className="font-semibold">{viewing.label}</strong> with you. Say "this" and I will know.
          </span>
        </p>
      )}

      <div ref={scrollRef} onScroll={(e) => e.currentTarget.scrollTop < 24 && loadOlder()} className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-4 py-3">
        {conversation.hasNextPage && (
          <Button variant="ghost" size="sm" className="self-center" loading={conversation.isFetchingNextPage} onClick={loadOlder}>
            Show earlier messages
          </Button>
        )}
        {conversation.isPending && (
          <p className="flex items-center gap-2 text-sm text-ink-muted">
            <Spinner className="h-4 w-4" />
            Opening your conversation…
          </p>
        )}
        {past.length > 0 && (
          <ol aria-label="Earlier conversation" className="flex flex-col gap-3">
            {past.map((turn, i) => renderTurn(turn, past[i - 1]))}
          </ol>
        )}
        <ol aria-label="Conversation" aria-live="polite" aria-relevant="additions" className="flex flex-col gap-3">
          {empty && (
            <li className="flex flex-col gap-2 text-sm text-ink-muted">
              <p>Type or speak a request. I will answer from your mail, and ask before I do anything.</p>
              <ul className="flex flex-col gap-1">
                {examples.map((example) => (
                  <li key={example}>
                    <button type="button" onClick={() => ask(example)} className="rounded-xs text-left text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover">
                      {example}
                    </button>
                  </li>
                ))}
              </ul>
            </li>
          )}
          {live.map((turn, i) => renderTurn(turn, i === 0 ? past[past.length - 1] : live[i - 1]))}
          {(command.isPending || transcribe.isPending) && (
            <li className="flex items-center gap-2 text-sm text-ink-muted">
              <Spinner className="h-4 w-4" />
              {transcribe.isPending ? 'Listening back…' : 'Thinking…'}
            </li>
          )}
        </ol>
      </div>

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
            placeholder={viewing.label === 'this email' ? `Ask ${assistantName} about this email` : `Ask ${assistantName} anything about your mail`}
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
