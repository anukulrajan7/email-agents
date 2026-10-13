import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Draft, DraftPatch } from '../../api/types';
import { messageKey } from '../inbox/hooks';

export function useGenerateDraft(ws: string, messageId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<{ status: 'queued' }>(`/workspaces/${ws}/messages/${messageId}/draft`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: messageKey(ws, messageId) }),
  });
}

export function useUpdateDraft(ws: string, messageId: string, draftId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (patch: DraftPatch) => api<Draft>(`/workspaces/${ws}/drafts/${draftId}`, { method: 'PATCH', body: patch }),
    onSuccess: () => qc.invalidateQueries({ queryKey: messageKey(ws, messageId) }),
  });
}
