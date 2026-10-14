import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Category, Classification, ClassificationPatch, DeleteResult, MessageDetail, MessageSummary, Paged } from '../../api/types';

const PAGE_SIZE = 25;
export const messagesKey = (ws: string) => ['ws', ws, 'messages'] as const;
export const messageKey = (ws: string, id: string) => ['ws', ws, 'message', id] as const;

export type MessageSort = 'importance' | 'recent';

export type ReplyFilter = 'awaiting' | 'replied';

export function useMessages(ws: string, filters: { category?: Category; q?: string; sort?: MessageSort; reply?: ReplyFilter }) {
  return useInfiniteQuery({
    queryKey: [...messagesKey(ws), filters],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      api<Paged<MessageSummary>>(`/workspaces/${ws}/messages`, {
        query: { cursor: pageParam, category: filters.category, q: filters.q, sort: filters.sort, reply: filters.reply, limit: PAGE_SIZE },
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

/** Reads an email right away when its summary isn't ready (older mail is read only when opened). */
export function useUnderstandNow(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<{ insight: unknown }>(`/workspaces/${ws}/messages/${id}/insight`, { method: 'POST' }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: messageKey(ws, id) });
      void qc.invalidateQueries({ queryKey: messagesKey(ws) });
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

/** Deletes emails from Post Room only (the Gmail copy stays): the given ones, or the whole conversation of one. */
export function useDeleteMessages(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (target: { ids: string[]; wholeThread?: boolean }) => {
      const [first] = target.ids;
      if (target.wholeThread && first !== undefined) {
        return api<DeleteResult>(`/workspaces/${ws}/messages/${first}/thread`, { method: 'DELETE' });
      }
      return api<DeleteResult>(`/workspaces/${ws}/messages/delete`, { method: 'POST', body: { ids: target.ids } });
    },
    onSuccess: (_d, target) => {
      for (const id of target.ids) qc.removeQueries({ queryKey: messageKey(ws, id) });
      void qc.invalidateQueries({ queryKey: messagesKey(ws) });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'contacts'] });
    },
  });
}

export const DELETE_NOTE = 'Your Gmail copy stays.';
