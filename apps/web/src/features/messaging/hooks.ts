import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Conversation, InboxItem, SentItem } from './types';

export const MESSAGE_POLL_MS = 30_000;

const key = (ws: string) => ['ws', ws, 'messaging'] as const;

export function useInApp(ws: string) {
  return useQuery({
    queryKey: [...key(ws), 'inbox'],
    queryFn: () => api<{ items: InboxItem[]; unread: number }>(`/workspaces/${ws}/in-app`),
    refetchInterval: MESSAGE_POLL_MS,
  });
}

export function useUnreadCount(ws: string): number {
  return useInApp(ws).data?.unread ?? 0;
}

export function useSentByMeera(ws: string) {
  return useQuery({
    queryKey: [...key(ws), 'sent'],
    queryFn: () => api<{ items: SentItem[] }>(`/workspaces/${ws}/in-app/sent`),
    refetchInterval: MESSAGE_POLL_MS,
  });
}

export function useConversations(ws: string) {
  return useQuery({
    queryKey: [...key(ws), 'conversations'],
    queryFn: () => api<{ items: Conversation[] }>(`/workspaces/${ws}/conversations`),
    refetchInterval: MESSAGE_POLL_MS,
  });
}

function useAction<V>(ws: string, run: (v: V) => Promise<unknown>) {
  const qc = useQueryClient();
  return useMutation({ mutationFn: run, onSuccess: () => qc.invalidateQueries({ queryKey: key(ws) }) });
}

export const useMarkRead = (ws: string) => useAction(ws, (id: string) => api(`/workspaces/${ws}/in-app/${id}/read`, { method: 'POST' }));
export const useReply = (ws: string) =>
  useAction(ws, ({ id, body }: { id: string; body: string }) => api(`/workspaces/${ws}/in-app/${id}/reply`, { method: 'POST', body: { body } }));
export const useStop = (ws: string) => useAction(ws, (id: string) => api(`/workspaces/${ws}/in-app/sent/${id}/stop`, { method: 'POST' }));
export const useApproveSend = (ws: string) => useAction(ws, (id: string) => api(`/workspaces/${ws}/in-app/sent/${id}/approve`, { method: 'POST' }));
export const useStopWatching = (ws: string) => useAction(ws, (id: string) => api(`/workspaces/${ws}/conversations/${id}/close`, { method: 'POST' }));
export const useTicketStatus = (ws: string) =>
  useAction(ws, ({ id, status }: { id: string; status: string }) =>
    api(`/workspaces/${ws}/support-tickets/${id}/status`, { method: 'POST', body: { status } }),
  );
