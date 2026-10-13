import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Draft, DraftPatch, SafetyReport, ScheduledSend } from '../../api/types';
import { messageKey, messagesKey } from '../inbox/hooks';

export function useGenerateDraft(ws: string, messageId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (opts?: { quality: 'premium' }) =>
      api<{ status: 'queued' }>(`/workspaces/${ws}/messages/${messageId}/draft`, { method: 'POST', body: opts }),
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

export function useSaveProviderDraft(ws: string, messageId: string, draftId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (body_text?: string) => {
      if (body_text !== undefined) {
        await api<Draft>(`/workspaces/${ws}/drafts/${draftId}`, { method: 'PATCH', body: { body_text, status: 'edited' } });
      }
      return api<{ status: 'saved'; provider_draft_id: string }>(`/workspaces/${ws}/drafts/${draftId}/provider-draft`, {
        method: 'POST',
      });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: messageKey(ws, messageId) }),
  });
}

export function useSendDraft(ws: string, messageId: string, draftId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (body_text?: string) => {
      if (body_text !== undefined) {
        await api<Draft>(`/workspaces/${ws}/drafts/${draftId}`, { method: 'PATCH', body: { body_text, status: 'edited' } });
      }
      return api<ScheduledSend>(`/workspaces/${ws}/drafts/${draftId}/send`, {
        method: 'POST',
        body: { confirm: true },
      });
    },
    onSuccess: () =>
      Promise.all([
        qc.invalidateQueries({ queryKey: messageKey(ws, messageId) }),
        qc.invalidateQueries({ queryKey: messagesKey(ws) }),
      ]),
  });
}

export function useCheckDraft(ws: string, draftId: string) {
  return useMutation({
    mutationFn: (body_text: string) =>
      api<SafetyReport>(`/workspaces/${ws}/drafts/${draftId}/check`, { method: 'POST', body: { body_text } }),
  });
}

export function useUndoSend(ws: string, messageId: string, draftId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<{ status: 'cancelled' }>(`/workspaces/${ws}/drafts/${draftId}/undo-send`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: messageKey(ws, messageId) }),
  });
}
