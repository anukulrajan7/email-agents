import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Category, Classification, ClassificationPatch, MessageDetail, MessageSummary, Paged } from '../../api/types';

const PAGE_SIZE = 25;
export const messagesKey = (ws: string) => ['ws', ws, 'messages'] as const;
export const messageKey = (ws: string, id: string) => ['ws', ws, 'message', id] as const;

export function useMessages(ws: string, filters: { category?: Category; q?: string }) {
  return useInfiniteQuery({
    queryKey: [...messagesKey(ws), filters],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      api<Paged<MessageSummary>>(`/workspaces/${ws}/messages`, {
        query: { cursor: pageParam, category: filters.category, q: filters.q, limit: PAGE_SIZE },
      }),
    getNextPageParam: (last) => last.next_cursor ?? undefined,
    refetchInterval: 5000,
  });
}

export function useMessage(ws: string, id: string | undefined, pollFast = false) {
  return useQuery({
    queryKey: messageKey(ws, id ?? ''),
    queryFn: () => api<MessageDetail>(`/workspaces/${ws}/messages/${id}`),
    enabled: id !== undefined,
    refetchInterval: (q) => {
      const status = q.state.data?.processing_status;
      return pollFast || status === 'pending' || status === 'normalized' ? 3000 : false;
    },
  });
}

export function useUpdateClassification(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (patch: ClassificationPatch) =>
      api<Classification>(`/workspaces/${ws}/messages/${id}/classification`, { method: 'PATCH', body: patch }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: messageKey(ws, id) });
      void qc.invalidateQueries({ queryKey: messagesKey(ws) });
    },
  });
}

export function useApproveClassification(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<Classification>(`/workspaces/${ws}/messages/${id}/classification/approve`, { method: 'POST' }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: messageKey(ws, id) });
      void qc.invalidateQueries({ queryKey: messagesKey(ws) });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'labels'] });
    },
  });
}

export function useReprocess(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (messageId: string) =>
      api<{ status: 'queued' }>(`/workspaces/${ws}/messages/${messageId}/reprocess`, { method: 'POST' }),
    onSuccess: (_d, messageId) => {
      void qc.invalidateQueries({ queryKey: messageKey(ws, messageId) });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'workflow-runs'] });
      void qc.invalidateQueries({ queryKey: messagesKey(ws) });
    },
  });
}
