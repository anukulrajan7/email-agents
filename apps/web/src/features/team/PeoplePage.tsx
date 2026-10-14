import { useState } from 'react';
import { Badge, Button, Card, ConfirmDialog, EmptyState, Page, PageHeader, SkeletonList, Table, Td, Th, useToast } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { ImportPanel } from './ImportPanel';
import { PersonForm } from './PersonForm';
import { useArchivePerson, useImportPeople, usePeople, useTeamAccess } from './hooks';
import type { Person } from './types';

export function PeoplePage() {
  const { ws } = useWorkspace();
  const people = usePeople(ws);
  const access = useTeamAccess(ws);
  const archive = useArchivePerson(ws);
  const importer = useImportPeople(ws);
  const notify = useToast();
  const [editing, setEditing] = useState<Person | 'new'>();
  const [removing, setRemoving] = useState<Person>();
  const [showImport, setShowImport] = useState(false);
  const staffing = access.data?.staffing ?? false;
  const items = people.data?.items ?? [];
  const canEdit = people.data?.can_edit ?? false;
  const nameOf = (id: string | null) => items.find((p) => p.id === id)?.name ?? '–';

  return (
    <Page className="gap-6">
      <PageHeader
        kicker={staffing ? 'HRMS · People' : 'Team · People'}
        title="People"
        description="Everyone on your team with their emails, manager, joining date and targets. Performance numbers and alert escalation both follow this list."
        actions={
          canEdit ? (
            <>
              <Button variant="secondary" onClick={() => setShowImport((s) => !s)}>
                Import CSV
              </Button>
              <Button onClick={() => setEditing('new')}>Add person</Button>
            </>
          ) : undefined
        }
      />
      <ErrorNote error={people.error ?? archive.error} />
      {people.isLoading && <SkeletonList rows={4} label="Loading people" />}
      {people.data && !canEdit && (
        <p className="text-sm text-ink-muted">You can see yourself and the people who report to you. Only the workspace's super admin can change the directory.</p>
      )}
      {showImport && canEdit && (
        <ImportPanel
          importer={importer}
          label="People CSV"
          columns={`name,email,role,manager_email,joining_date,daily_target,weekly_target${staffing ? ',employee_type,client' : ''}`}
        />
      )}
      {people.data &&
        (items.length === 0 ? (
          <Card>
            <EmptyState
              title="No one in the directory yet"
              description={
                canEdit
                  ? 'Start by adding the people on your team, or import them from a CSV. Give each person their email and, if they report to someone, a manager.'
                  : 'The super admin has not added you to the directory yet. Ask them to add your email address.'
              }
              action={canEdit ? <Button onClick={() => setEditing('new')}>Add the first person</Button> : undefined}
            />
          </Card>
        ) : (
          <div className="rise" style={riseStyle(1)}>
            <Table>
              <thead>
                <tr>
                  <Th>Name</Th>
                  <Th>Emails</Th>
                  <Th>Role</Th>
                  <Th>Manager</Th>
                  <Th>Joined</Th>
                  <Th>Targets (day / week)</Th>
                  {staffing && <Th>Type</Th>}
                  {canEdit && <Th>
                    <span className="sr-only">Actions</span>
                  </Th>}
                </tr>
              </thead>
              <tbody>
                {items.map((p) => (
                  <tr key={p.id}>
                    <Td className="font-semibold text-ink">{p.name}</Td>
                    <Td>{p.emails.join(', ')}</Td>
                    <Td>{p.role ?? '–'}</Td>
                    <Td>{nameOf(p.manager_id)}</Td>
                    <Td>{p.joining_date ?? '–'}</Td>
                    <Td>
                      {p.daily_target ?? '–'} / {p.weekly_target ?? '–'}
                    </Td>
                    {staffing && (
                      <Td>
                        <Badge tone={p.employee_type === 'billable' ? 'info' : 'neutral'}>{p.employee_type === 'billable' ? 'Billable' : 'Non-billable'}</Badge>
                      </Td>
                    )}
                    {canEdit && (
                      <Td className="whitespace-nowrap">
                        <Button size="sm" variant="ghost" aria-label={`Edit ${p.name}`} onClick={() => setEditing(p)}>
                          Edit
                        </Button>
                        <Button size="sm" variant="ghost" aria-label={`Remove ${p.name}`} onClick={() => setRemoving(p)}>
                          Remove
                        </Button>
                      </Td>
                    )}
                  </tr>
                ))}
              </tbody>
            </Table>
          </div>
        ))}
      {editing && <PersonForm ws={ws} people={items} staffing={staffing} {...(editing === 'new' ? {} : { person: editing })} onClose={() => setEditing(undefined)} />}
      {removing && (
        <ConfirmDialog
          title={`Remove ${removing.name}?`}
          confirmLabel="Remove"
          busy={archive.isPending}
          onCancel={() => setRemoving(undefined)}
          onConfirm={() =>
            archive.mutate(removing.id, {
              onSuccess: () => {
                notify('success', `${removing.name} removed.`);
                setRemoving(undefined);
              },
            })
          }
        >
          Their past numbers are kept. Their email addresses are freed and they leave the manager chain.
        </ConfirmDialog>
      )}
    </Page>
  );
}
