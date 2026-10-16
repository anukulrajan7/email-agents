import { useId, useState, type FormEvent } from 'react';
import { Badge, Button, Card, Dialog, Input, Label, Select, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { usePeople } from '../team/hooks';
import { useAddIdentityAsPerson, useIdentities, useLinkIdentity, useSetIdentityStatus, useUndoIdentityChange } from './hooks';
import { hoursLabel, periodLabel } from './timesheetFormat';
import type { IdentityChange, UnknownPerson } from './types';

const sheets = (n: number) => `${n} ${n === 1 ? 'sheet' : 'sheets'}`;

function changeText(c: IdentityChange): string {
  const who = c.name ?? 'A name';
  switch (c.kind) {
    case 'link':
      return `Linked ${who} to ${c.person_name ?? 'a person'} (${sheets(c.record_count)})`;
    case 'mark_external':
      return `Marked ${who} as outside staff`;
    case 'dismiss':
      return `Marked ${who} as not a person`;
    default:
      return `${who}: ${c.kind.replace('_', ' ')}`;
  }
}

function AddPersonDialog({ ws, person, onClose }: { ws: string; person: UnknownPerson; onClose: () => void }) {
  const uid = useId();
  const add = useAddIdentityAsPerson(ws);
  const notify = useToast();
  const [name, setName] = useState(person.name ?? '');
  const [email, setEmail] = useState('');
  const [code, setCode] = useState(person.employee_code ?? '');

  const submit = (e: FormEvent) => {
    e.preventDefault();
    add.mutate(
      { id: person.id, body: { name: name.trim(), email: email.trim(), employee_code: code.trim() } },
      {
        onSuccess: () => {
          notify('success', `${name.trim() || 'The person'} was added to People, and ${sheets(person.record_count)} now belong to them.`);
          onClose();
        },
      },
    );
  };

  return (
    <Dialog title={`Add ${person.name ?? 'this person'} to People`} onClose={onClose}>
      <form onSubmit={submit} className="flex flex-col gap-3">
        <p className="text-sm text-ink-muted">
          They are added to your people list, with their {sheets(person.record_count)}. This does not give them a login.
        </p>
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-name`}>Name</Label>
          <Input id={`${uid}-name`} value={name} onChange={(e) => setName(e.target.value)} required />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-email`}>Email address</Label>
          <Input id={`${uid}-email`} type="email" value={email} onChange={(e) => setEmail(e.target.value)} required />
        </div>
        <div className="flex flex-col gap-1">
          <Label htmlFor={`${uid}-code`}>Employee ID (optional)</Label>
          <Input id={`${uid}-code`} value={code} onChange={(e) => setCode(e.target.value)} />
        </div>
        <ErrorNote error={add.error} />
        <div className="flex gap-2">
          <Button type="submit" loading={add.isPending}>
            Add person
          </Button>
          <Button variant="secondary" onClick={onClose}>
            Cancel
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

function PersonCard({ ws, person }: { ws: string; person: UnknownPerson }) {
  const uid = useId();
  const people = usePeople(ws);
  const link = useLinkIdentity(ws);
  const status = useSetIdentityStatus(ws);
  const notify = useToast();
  const [chosen, setChosen] = useState('');
  const [rememberName, setRememberName] = useState(false);
  const [adding, setAdding] = useState(false);
  const label = person.name ?? 'No name written';
  const linkTo = (personId: string, personName: string) =>
    link.mutate({ id: person.id, person_id: personId, remember_name: rememberName }, { onSuccess: () => notify('success', `${label} is now ${personName}.`) });
  const mark = (action: 'external' | 'dismiss', text: string) => status.mutate({ id: person.id, action }, { onSuccess: () => notify('success', text) });

  return (
    <li className="flex flex-col gap-3 rounded-sm border border-rule p-3" aria-label={label}>
      <div className="flex flex-wrap items-baseline gap-2">
        <span className="font-semibold text-ink">{label}</span>
        {person.employee_code && <Badge>ID {person.employee_code}</Badge>}
        <span className="text-sm text-ink-muted">
          {sheets(person.record_count)}
          {person.sender ? `, sent from ${person.sender}` : ''}
        </span>
      </div>
      <ul className="flex flex-col gap-0.5 text-sm text-ink-muted" aria-label={`Sheets for ${label}`}>
        {person.records.map((r) => (
          <li key={r.id}>
            {periodLabel(r.period_start, r.period_end)} · {hoursLabel(r.total_hours)}
            {r.file ? ` · ${r.file}` : ''}
            {r.possible_duplicate && (
              <>
                {' '}
                <Badge tone="warning">Possible duplicate</Badge>
              </>
            )}
          </li>
        ))}
      </ul>
      {person.suggestions.length > 0 && (
        <div className="flex flex-col gap-2">
          <p className="text-sm text-ink">It might be one of these. Nothing is linked until you say so.</p>
          <ul className="flex flex-col gap-2">
            {person.suggestions.map((s) => (
              <li key={s.person_id} className="flex flex-wrap items-center gap-2 text-sm">
                <Button size="sm" variant="secondary" disabled={link.isPending} aria-label={`Link ${label} to ${s.name}`} onClick={() => linkTo(s.person_id, s.name)}>
                  This is {s.name}
                </Button>
                <span className="text-ink-muted">{s.reason}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <div className="flex flex-wrap items-center gap-2">
        <Select aria-label={`Link ${label} to a person`} className="max-w-64" value={chosen} onChange={(e) => setChosen(e.target.value)}>
          <option value="">Someone else in People</option>
          {(people.data?.items ?? []).map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </Select>
        <Button
          size="sm"
          disabled={!chosen}
          loading={link.isPending}
          aria-label={`Link ${label} to the chosen person`}
          onClick={() => linkTo(chosen, (people.data?.items ?? []).find((p) => p.id === chosen)?.name ?? 'that person')}
        >
          Link
        </Button>
        <label className="flex items-center gap-2 text-sm text-ink" htmlFor={`${uid}-remember`}>
          <input id={`${uid}-remember`} type="checkbox" checked={rememberName} onChange={(e) => setRememberName(e.target.checked)} />
          Also match this name next time
        </label>
      </div>
      <div className="flex flex-wrap gap-2">
        <Button size="sm" variant="secondary" onClick={() => setAdding(true)}>
          Add as a person
        </Button>
        <Button size="sm" variant="ghost" loading={status.isPending} onClick={() => mark('external', `${label} is kept as outside staff.`)}>
          They work for a client
        </Button>
        <Button size="sm" variant="ghost" loading={status.isPending} onClick={() => mark('dismiss', `${label} is kept as not a person.`)}>
          Not a person
        </Button>
      </div>
      <ErrorNote error={link.error ?? status.error} />
      {adding && <AddPersonDialog ws={ws} person={person} onClose={() => setAdding(false)} />}
    </li>
  );
}

/** Names that came on timesheets and are not in People: kept as written, matched only by a person's choice. */
export function UnknownPeople({ ws }: { ws: string }) {
  const list = useIdentities(ws, true);
  const undo = useUndoIdentityChange(ws);
  const notify = useToast();
  const data = list.data;
  if (!data?.enabled) return <ErrorNote error={list.error} />;
  const items = data.items ?? [];
  const recent = (data.recent ?? []).filter((c) => c.can_undo);
  if (items.length === 0 && recent.length === 0) return null;

  return (
    <Card>
      <section aria-labelledby={`unknown-${ws}`} className="flex flex-col gap-4">
        <div>
          <h2 id={`unknown-${ws}`} className="text-base font-semibold text-ink">
            People we don't know yet
          </h2>
          <p className="mt-1 text-sm text-ink-muted">
            These names came on timesheets but are not in People. Every sheet is kept exactly as written, and nothing is matched to a person by a similar name: you decide. The sender gets a short receipt now, and the confirmation once you have said who this is.
          </p>
        </div>
        {items.length > 0 && (
          <ul className="flex flex-col gap-3" aria-label="Names to sort out">
            {items.map((p) => (
              <PersonCard key={p.id} ws={ws} person={p} />
            ))}
          </ul>
        )}
        {recent.length > 0 && (
          <div className="flex flex-col gap-2">
            <h3 className="text-sm font-semibold text-ink">Just decided</h3>
            <ul className="flex flex-col gap-2" aria-label="Recent decisions">
              {recent.map((c) => (
                <li key={c.id} className="flex flex-wrap items-center gap-2 text-sm text-ink-muted">
                  <span>{changeText(c)}</span>
                  <Button size="sm" variant="ghost" loading={undo.isPending} aria-label={`Undo: ${changeText(c)}`} onClick={() => undo.mutate(c.id, { onSuccess: () => notify('success', 'Undone. The sheets are back under "People we don\'t know yet" and marked for a check.') })}>
                    Undo
                  </Button>
                </li>
              ))}
            </ul>
            <p className="text-xs text-ink-muted">Undo puts the sheets back to "who is this". A reply that already went out is not taken back.</p>
            <ErrorNote error={undo.error} />
          </div>
        )}
      </section>
    </Card>
  );
}
