import { useId, useState } from 'react';
import { Button, Dialog, Input, Label, SegmentedControl, Switch, Textarea, labelColors, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { cn } from '../../lib/cn';
import { AgentAvatar } from './AgentAvatar';
import {
  AVATARS,
  COLORS,
  COLOR_NAME,
  MAX_NAME,
  MAX_PERSONA,
  MAX_SIGNATURE,
  QUALITY_OPTIONS,
  SEND_OPTIONS,
  TASK_OPTIONS,
} from './agentLabels';
import { useSaveAgent } from './hooks';
import { TestOnEmail } from './TestOnEmail';
import type { AgentInput, AgentQuality, AgentTask, SendMode, WorkspaceAgent } from './types';

const PERSONA_PLACEHOLDER =
  'Who is your agent and how does it talk? Example: Warm and upbeat, short paragraphs, always offers a next step. ' +
  'Never discusses salary or makes promises about offers.';

export function AgentEditor({
  ws,
  agent,
  standardGuidance,
  onClose,
}: {
  ws: string;
  agent: WorkspaceAgent;
  standardGuidance: string;
  onClose: () => void;
}) {
  const uid = useId();
  const notify = useToast();
  const save = useSaveAgent(ws);
  const [name, setName] = useState(agent.name);
  const [avatar, setAvatar] = useState(agent.avatar);
  const [color, setColor] = useState(agent.color);
  const [persona, setPersona] = useState(agent.persona);
  const [signature, setSignature] = useState(agent.signature ?? '');
  const [tasks, setTasks] = useState<AgentTask[]>(agent.tasks);
  const [draft, setDraft] = useState(agent.permissions.draft);
  const [send, setSend] = useState<SendMode>(agent.permissions.send);
  const [quality, setQuality] = useState<AgentQuality>(agent.quality);
  const [enabled, setEnabled] = useState(agent.enabled);
  const shownName = name.trim() || agent.name;

  const input: AgentInput = {
    name: name.trim(),
    avatar: avatar.trim(),
    color,
    persona,
    signature: signature.trim() || null,
    tasks,
    permissions: { draft, send },
    quality,
    enabled,
  };

  const toggleTask = (t: AgentTask) => setTasks((cur) => (cur.includes(t) ? cur.filter((x) => x !== t) : [...cur, t]));

  return (
    <Dialog title={`Edit ${agent.name}`} variant="drawer" onClose={onClose} dismissible={!save.isPending}>
      <form
        className="flex flex-col gap-5"
        onSubmit={(e) => {
          e.preventDefault();
          save.mutate(input, {
            onSuccess: () => {
              notify('success', `${shownName} is updated.`);
              onClose();
            },
          });
        }}
      >
        <div className="flex items-center gap-3">
          <AgentAvatar agent={{ avatar: avatar || '?', color }} size="lg" />
          <div className="flex flex-1 flex-col gap-1">
            <Label htmlFor={`${uid}-name`}>Name</Label>
            <Input id={`${uid}-name`} required maxLength={MAX_NAME} value={name} onChange={(e) => setName(e.target.value)} />
          </div>
        </div>

        <fieldset className="flex flex-col gap-2">
          <legend className="kicker mb-1">Avatar</legend>
          <div className="flex flex-wrap gap-2">
            {AVATARS.map((a) => (
              <button
                key={a}
                type="button"
                aria-pressed={avatar === a}
                aria-label={`Avatar ${a}`}
                onClick={() => setAvatar(a)}
                className={cn(
                  'flex h-9 w-9 items-center justify-center rounded-full border text-lg',
                  'focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
                  avatar === a ? 'border-ink bg-raised' : 'border-rule hover:border-ink',
                )}
              >
                {a}
              </button>
            ))}
          </div>
          <div className="flex items-center gap-2">
            <Label htmlFor={`${uid}-initials`} className="normal-case tracking-normal">
              Or initials
            </Label>
            <Input
              id={`${uid}-initials`}
              className="w-20"
              maxLength={2}
              value={AVATARS.includes(avatar) ? '' : avatar}
              onChange={(e) => setAvatar(e.target.value.toUpperCase())}
            />
          </div>
        </fieldset>

        <fieldset className="flex flex-col gap-2">
          <legend className="kicker mb-1">Colour</legend>
          <div className="flex flex-wrap gap-2">
            {COLORS.map((c) => (
              <button
                key={c}
                type="button"
                aria-pressed={color === c}
                aria-label={COLOR_NAME[c]}
                title={COLOR_NAME[c]}
                onClick={() => setColor(c)}
                className={cn(
                  'h-8 w-8 rounded-full border-2 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
                  labelColors[c]?.swatch,
                  color === c ? 'border-ink ring-2 ring-raised ring-inset' : 'border-transparent',
                )}
              />
            ))}
          </div>
        </fieldset>

        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-persona`}>Who {shownName} is and how it writes</Label>
          <Textarea
            id={`${uid}-persona`}
            rows={9}
            maxLength={MAX_PERSONA}
            placeholder={PERSONA_PLACEHOLDER}
            value={persona}
            onChange={(e) => setPersona(e.target.value)}
          />
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Button size="sm" variant="secondary" onClick={() => setPersona(standardGuidance)}>
              Start from the standard guidance
            </Button>
            <span className="font-mono text-xs text-ink-muted">
              {persona.length} / {MAX_PERSONA}
            </span>
          </div>
          <p className="text-sm text-ink-muted">Our safety rules always come first: {shownName} never promises money, offers or dates, and never rejects anyone.</p>
        </div>

        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-signature`}>Default signature</Label>
          <Input
            id={`${uid}-signature`}
            maxLength={MAX_SIGNATURE}
            placeholder="Example: The Acme Team"
            value={signature}
            onChange={(e) => setSignature(e.target.value)}
          />
          <p className="text-sm text-ink-muted">People can set their own sign-off in their profile.</p>
        </div>

        <fieldset className="flex flex-col gap-2">
          <legend className="kicker mb-1">Jobs</legend>
          {TASK_OPTIONS.map((t) => (
            <label key={t.value} className="flex items-center gap-2 text-sm text-ink">
              <input type="checkbox" className="h-4 w-4 accent-[var(--color-accent)]" checked={tasks.includes(t.value)} onChange={() => toggleTask(t.value)} />
              {t.label}
            </label>
          ))}
        </fieldset>

        <fieldset className="flex flex-col gap-3">
          <legend className="kicker mb-1">Permissions</legend>
          <div className="flex items-center justify-between gap-4">
            <span className="text-sm text-ink">Write reply drafts</span>
            <Switch checked={draft} label="Write reply drafts" onChange={setDraft} />
          </div>
          <div className="flex flex-col gap-2">
            <span className="text-sm text-ink">Sending</span>
            <SegmentedControl label="Sending" options={SEND_OPTIONS} value={send} onChange={setSend} />
            {send === 'auto' && (
              <p role="note" className="rounded-sm border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
                {shownName} may send routine replies without asking, but only for people whose profile says “Let routine replies go”,
                only to someone we have written to before, and never about money, rates, offers or rejections. Everything else still
                waits for a person. Submissions always need approval.
              </p>
            )}
          </div>
        </fieldset>

        <fieldset className="flex flex-col gap-2">
          <legend className="kicker mb-1">Quality</legend>
          <SegmentedControl label="Quality" options={QUALITY_OPTIONS} value={quality} onChange={setQuality} />
          <p className="text-sm text-ink-muted">{QUALITY_OPTIONS.find((q) => q.value === quality)?.hint}</p>
        </fieldset>

        <div className="flex items-center justify-between gap-4">
          <span className="text-sm text-ink">{shownName} is on</span>
          <Switch checked={enabled} label={`${shownName} is on`} onChange={setEnabled} />
        </div>

        <TestOnEmail ws={ws} agentName={shownName} agent={{ persona, signature: signature.trim() || null, quality }} />

        <ErrorNote error={save.error} />
        <div className="flex flex-wrap gap-2">
          <Button type="submit" loading={save.isPending} disabled={!name.trim()}>
            Save
          </Button>
          <Button variant="secondary" onClick={onClose} disabled={save.isPending}>
            Cancel
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
