import { useState } from 'react';
import { useNavigate } from 'react-router';
import { Button, ConfirmDialog, useToast } from '../../components/ui';
import { DELETE_NOTE, useDeleteMessages } from './hooks';

export function deletedToast(count: number): string {
  return `${count === 1 ? 'Email' : `${count} emails`} deleted from Post Room. ${DELETE_NOTE}`;
}

/** The reader's Delete action: removes this email (or its whole conversation) from Post Room only. */
export function DeleteEmailButton({ ws, messageId }: { ws: string; messageId: string }) {
  const [open, setOpen] = useState(false);
  const [wholeThread, setWholeThread] = useState(false);
  const del = useDeleteMessages(ws);
  const notify = useToast();
  const navigate = useNavigate();

  const close = () => {
    del.reset();
    setOpen(false);
    setWholeThread(false);
  };

  return (
    <>
      <Button variant="ghost" size="sm" onClick={() => setOpen(true)}>
        Delete
      </Button>
      {open && (
        <ConfirmDialog
          title="Delete from Post Room?"
          confirmLabel="Delete"
          busy={del.isPending}
          error={del.error}
          onCancel={close}
          onConfirm={() =>
            del.mutate(
              { ids: [messageId], wholeThread },
              {
                onSuccess: (res) => {
                  notify('success', deletedToast(res.deleted));
                  void navigate(`/w/${ws}/inbox`);
                },
              },
            )
          }
        >
          <p>{DELETE_NOTE} The email, its attachments, summaries and drafts are removed from Post Room, and it will not come back when Gmail syncs. This can&apos;t be undone.</p>
          <label className="mt-3 flex items-center gap-2 text-ink">
            <input type="checkbox" checked={wholeThread} onChange={(e) => setWholeThread(e.target.checked)} className="h-4 w-4 accent-accent" />
            Also delete the rest of this conversation
          </label>
        </ConfirmDialog>
      )}
    </>
  );
}
