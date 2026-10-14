import { useId, useState, type FormEvent } from 'react';
import { Button, Dialog, Input, Label, Select } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useSavePerson } from './hooks';
import type { EmployeeType, Person } from './types';

const toNumber = (v: string): number | null => (v.trim() === '' ? null : Number(v));

export function PersonForm({ ws, person, people, staffing, onClose }: { ws: string; person?: Person; people: Person[]; staffing: boolean; onClose: () => void }) {
  const uid = useId();
  const save = useSavePerson(ws);
  const [name, setName] = useState(person?.name ?? '');
  const [emails, setEmails] = useState(person?.emails.join(', ') ?? '');
  const [role, setRole] = useState(person?.role ?? '');
  const [manager, setManager] = useState(person?.manager_id ?? '');
  const [joined, setJoined] = useState(person?.joining_date ?? '');
  const [daily, setDaily] = useState(person?.daily_target?.toString() ?? '');
  const [weekly, setWeekly] = useState(person?.weekly_target?.toString() ?? '');
  const [type, setType] = useState<EmployeeType>(person?.employee_type ?? 'non_billable');
  const [client, setClient] = useState(person?.client ?? '');

  const submit = (e: FormEvent) => {
    e.preventDefault();
    save.mutate(
      {
        ...(person ? { id: person.id } : {}),
        body: {
          name: name.trim(),
          emails: emails.split(/[,;\s]+/).filter(Boolean),
          role: role.trim() || null,
          manager_id: manager || null,
          joining_date: joined || null,
          daily_target: toNumber(daily),
          weekly_target: toNumber(weekly),
          ...(staffing ? { employee_type: type, client: client.trim() || null } : {}),
        },
      },
      { onSuccess: onClose },
    );
  };

  const field = (label: string, input: React.ReactNode, id: string) => (
    <div className="flex flex-col gap-1">
      <Label htmlFor={`${uid}-${id}`}>{label}</Label>
      {input}
    </div>
  );

  return (
    <Dialog title={person ? `Edit ${person.name}` : 'Add a person'} onClose={onClose}>
      <form onSubmit={submit} className="flex flex-col gap-4">
        {field('Name', <Input id={`${uid}-name`} required value={name} onChange={(e) => setName(e.target.value)} />, 'name')}
        {field('Email addresses (comma separated)', <Input id={`${uid}-emails`} required value={emails} onChange={(e) => setEmails(e.target.value)} />, 'emails')}
        {field('Role', <Input id={`${uid}-role`} value={role} onChange={(e) => setRole(e.target.value)} />, 'role')}
        {field(
          'Manager',
          <Select id={`${uid}-manager`} value={manager} onChange={(e) => setManager(e.target.value)}>
            <option value="">No manager</option>
            {people
              .filter((p) => p.id !== person?.id)
              .map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
          </Select>,
          'manager',
        )}
        {field('Joining date', <Input id={`${uid}-joined`} type="date" value={joined} onChange={(e) => setJoined(e.target.value)} />, 'joined')}
        <div className="grid grid-cols-2 gap-3">
          {field('Daily target', <Input id={`${uid}-daily`} type="number" min={0} value={daily} onChange={(e) => setDaily(e.target.value)} />, 'daily')}
          {field('Weekly target', <Input id={`${uid}-weekly`} type="number" min={0} value={weekly} onChange={(e) => setWeekly(e.target.value)} />, 'weekly')}
        </div>
        {staffing && (
          <>
            {field(
              'Employee type',
              <Select id={`${uid}-type`} value={type} onChange={(e) => setType(e.target.value as EmployeeType)}>
                <option value="non_billable">Non-billable (full mailbox)</option>
                <option value="billable">Billable (HR mailbox only)</option>
              </Select>,
              'type',
            )}
            {field('Client', <Input id={`${uid}-client`} value={client} onChange={(e) => setClient(e.target.value)} />, 'client')}
          </>
        )}
        <ErrorNote error={save.error} />
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" loading={save.isPending}>
            {person ? 'Save changes' : 'Add person'}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
