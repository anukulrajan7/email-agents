import { useState } from 'react';
import { Button, Dialog, useToast } from '../../components/ui';
import { useMyProfile, useSaveMyProfile, useSkipMyProfile } from './hooks';
import { ProfileForm } from './ProfileForm';

/** After someone connects their mailbox: "Tell Meera about you". Skippable; editable later under Agents → My profile. */
export function OnboardingPrompt({ ws }: { ws: string }) {
  const mine = useMyProfile(ws);
  const save = useSaveMyProfile(ws);
  const skip = useSkipMyProfile(ws);
  const notify = useToast();
  const [later, setLater] = useState(false);
  const data = mine.data;
  if (!data?.needs_onboarding || !data.profile || later) return null;
  const name = data.agent?.name ?? 'the agent';

  return (
    <Dialog title={`Tell ${name} about you`} variant="drawer" onClose={() => setLater(true)} dismissible={!save.isPending}>
      <p className="text-sm text-ink-muted">
        You connected {data.mailboxes.join(', ') || 'your mailbox'}. A few details help {name} reply the way you would. You can change this any
        time under Agents → My profile.
      </p>
      <ProfileForm
        ws={ws}
        agentName={name}
        initial={data.profile}
        saving={save.isPending}
        error={save.error ?? skip.error}
        showTest={false}
        onSave={(input) => save.mutate(input, { onSuccess: () => notify('success', `${name} now knows how you work.`) })}
        extraActions={
          <Button variant="ghost" loading={skip.isPending} onClick={() => skip.mutate()}>
            Skip for now
          </Button>
        }
      />
    </Dialog>
  );
}
