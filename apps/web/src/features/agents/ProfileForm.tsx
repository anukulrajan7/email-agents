import { useId, useState, type ReactNode } from 'react';
import { Button, Input, Label, SegmentedControl, Select, Textarea } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import {
  APPROVAL_OPTIONS,
  AUDIENCE_OPTIONS,
  FORMALITY_OPTIONS,
  HANDLE_OPTIONS,
  LENGTH_OPTIONS,
  MAX_SIGNATURE,
} from './agentLabels';
import { TestOnEmail } from './TestOnEmail';
import type { Audience, Formality, Handle, ReplyLength, SendApproval, UserProfile, UserProfileInput } from './types';

function toggle<T>(list: T[], v: T): T[] {
  return list.includes(v) ? list.filter((x) => x !== v) : [...list, v];
}

function Checks<T extends string>({ legend, options, value, onChange }: { legend: string; options: { value: T; label: string }[]; value: T[]; onChange: (v: T[]) => void }) {
  return (
    <fieldset className="flex flex-col gap-2">
      <legend className="kicker mb-1">{legend}</legend>
      <div className="grid grid-cols-1 gap-x-4 gap-y-2 sm:grid-cols-2">
        {options.map((o) => (
          <label key={o.value} className="flex items-center gap-2 text-sm text-ink">
            <input type="checkbox" className="h-4 w-4 accent-[var(--color-accent)]" checked={value.includes(o.value)} onChange={() => onChange(toggle(value, o.value))} />
            {o.label}
          </label>
        ))}
      </div>
    </fieldset>
  );
}

/** The person's own profile: who they are, what they handle, how they write, and how much the agent may do. */
export function ProfileForm({
  ws,
  agentName,
  initial,
  saving,
  error,
  onSave,
  extraActions,
  showTest = true,
}: {
  ws: string;
  agentName: string;
  initial: UserProfile;
  saving: boolean;
  error: unknown;
  onSave: (input: UserProfileInput) => void;
  extraActions?: ReactNode;
  showTest?: boolean;
}) {
  const uid = useId();
  const [displayName, setDisplayName] = useState(initial.display_name);
  const [jobTitle, setJobTitle] = useState(initial.job_title);
  const [handles, setHandles] = useState<Handle[]>(initial.handles);
  const [handlesNote, setHandlesNote] = useState(initial.handles_note);
  const [ignoreNote, setIgnoreNote] = useState(initial.ignore_note);
  const [audiences, setAudiences] = useState<Audience[]>(initial.audiences);
  const [vips, setVips] = useState(initial.vips.join(', '));
  const [formality, setFormality] = useState<Formality>(initial.tone.formality);
  const [length, setLength] = useState<ReplyLength>(initial.tone.length);
  const [language, setLanguage] = useState(initial.tone.language);
  const [signOff, setSignOff] = useState(initial.tone.sign_off ?? '');
  const [hours, setHours] = useState(initial.working_hours);
  const [timezone, setTimezone] = useState(initial.timezone);
  const [neverDo, setNeverDo] = useState(initial.never_do);
  const [approval, setApproval] = useState<SendApproval>(initial.approvals.send_email);

  const input: UserProfileInput = {
    display_name: displayName.trim(),
    job_title: jobTitle.trim(),
    handles,
    handles_note: handlesNote.trim(),
    ignore_note: ignoreNote.trim(),
    audiences,
    vips: vips
      .split(/[\s,;]+/)
      .map((v) => v.trim())
      .filter(Boolean),
    tone: { formality, length, sign_off: signOff.trim() || null, language: language.trim() },
    working_hours: hours.trim(),
    timezone: timezone.trim(),
    never_do: neverDo.trim(),
    approvals: { send_email: approval },
  };

  return (
    <form
      className="flex flex-col gap-5"
      onSubmit={(e) => {
        e.preventDefault();
        onSave(input);
      }}
    >
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-name`}>Your name</Label>
          <Input id={`${uid}-name`} maxLength={60} placeholder="Priya" value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-title`}>Role or job title</Label>
          <Input id={`${uid}-title`} maxLength={80} placeholder="Senior Recruiter" value={jobTitle} onChange={(e) => setJobTitle(e.target.value)} />
        </div>
      </div>

      <Checks legend="What you handle" options={HANDLE_OPTIONS} value={handles} onChange={setHandles} />
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${uid}-handles`}>In your own words</Label>
        <Textarea
          id={`${uid}-handles`}
          rows={2}
          maxLength={500}
          placeholder="Java and React hiring for our enterprise clients"
          value={handlesNote}
          onChange={(e) => setHandlesNote(e.target.value)}
        />
      </div>
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${uid}-ignore`}>What {agentName} should leave to you</Label>
        <Textarea
          id={`${uid}-ignore`}
          rows={2}
          maxLength={500}
          placeholder="Payroll disputes, anything from legal"
          value={ignoreNote}
          onChange={(e) => setIgnoreNote(e.target.value)}
        />
      </div>

      <Checks legend="Who you usually email" options={AUDIENCE_OPTIONS} value={audiences} onChange={setAudiences} />
      <div className="flex flex-col gap-1">
        <Label htmlFor={`${uid}-vips`}>Important people or domains</Label>
        <Input id={`${uid}-vips`} placeholder="ceo@client.com, capgemini.com" value={vips} onChange={(e) => setVips(e.target.value)} />
        <p className="text-sm text-ink-muted">{agentName} takes extra care with these. Separate with commas.</p>
      </div>

      <fieldset className="flex flex-col gap-3">
        <legend className="kicker mb-1">How you write</legend>
        <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-formality`}>Formality</Label>
            <Select id={`${uid}-formality`} value={formality} onChange={(e) => setFormality(e.target.value as Formality)}>
              {FORMALITY_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </Select>
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-length`}>Length</Label>
            <Select id={`${uid}-length`} value={length} onChange={(e) => setLength(e.target.value as ReplyLength)}>
              {LENGTH_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </Select>
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-language`}>Language</Label>
            <Input id={`${uid}-language`} maxLength={40} placeholder="English" value={language} onChange={(e) => setLanguage(e.target.value)} />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${uid}-signoff`}>Sign-off</Label>
            <Input id={`${uid}-signoff`} maxLength={MAX_SIGNATURE} placeholder="Priya S., Acme" value={signOff} onChange={(e) => setSignOff(e.target.value)} />
          </div>
        </div>
      </fieldset>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-hours`}>Working hours</Label>
          <Input id={`${uid}-hours`} maxLength={80} placeholder="Mon–Fri, 9:00–18:00" value={hours} onChange={(e) => setHours(e.target.value)} />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-tz`}>Time zone</Label>
          <Input id={`${uid}-tz`} maxLength={60} placeholder="Asia/Kolkata" value={timezone} onChange={(e) => setTimezone(e.target.value)} />
        </div>
      </div>

      <div className="flex flex-col gap-1">
        <Label htmlFor={`${uid}-never`}>Things {agentName} must never do for you</Label>
        <Textarea
          id={`${uid}-never`}
          rows={3}
          maxLength={1000}
          placeholder="Never share salary bands. Never reply to candidates after 8 pm."
          value={neverDo}
          onChange={(e) => setNeverDo(e.target.value)}
        />
      </div>

      <fieldset className="flex flex-col gap-2">
        <legend className="kicker mb-1">Before {agentName} sends a reply for you</legend>
        <SegmentedControl label="Before sending" options={APPROVAL_OPTIONS} value={approval} onChange={setApproval} />
        <p className="text-sm text-ink-muted">
          {approval === 'routine_ok'
            ? `Routine replies to people you've written to before can go without asking, if an admin allows it. Anything about money, offers or rejections still waits for you.`
            : `${agentName} always asks you before anything is sent.`}
        </p>
      </fieldset>

      {showTest && <TestOnEmail ws={ws} agentName={agentName} profile={input} />}

      <ErrorNote error={error} />
      <div className="flex flex-wrap items-center gap-2">
        <Button type="submit" loading={saving}>
          Save profile
        </Button>
        {extraActions}
      </div>
    </form>
  );
}
