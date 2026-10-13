import { useAgent } from '../agents/hooks';
import { ASSISTANT_NAME } from './labels';
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { CommandResponse, ConfirmResponse, HistoryPage, PageContext } from './types';

export function useCommand(ws: string) {
  return useMutation({
    mutationFn: ({ text, context }: { text: string; context: PageContext }) =>
      api<CommandResponse>(`/workspaces/${ws}/assistant/command`, { method: 'POST', body: { text, context } }),
  });
}

/** The person's one running conversation, newest page first; `fetchNextPage` loads older messages. */
export function useConversation(ws: string) {
  return useInfiniteQuery({
    queryKey: ['assistant', ws, 'history'],
    initialPageParam: undefined as number | undefined,
    queryFn: ({ pageParam }) =>
      api<HistoryPage>(`/workspaces/${ws}/assistant/history`, { query: { limit: 20, before: pageParam } }),
    getNextPageParam: (last) => last.next_before ?? undefined,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
  });
}

export function useConfirmPlan(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (planItemId: string) =>
      api<ConfirmResponse>(`/workspaces/${ws}/assistant/confirm`, { method: 'POST', body: { plan_item_id: planItemId } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['ws', ws] }),
  });
}

export function useTranscribe(ws: string) {
  return useMutation({
    mutationFn: ({ blob, durationMs }: { blob: Blob; durationMs: number }) => {
      const form = new FormData();
      form.append('audio', blob, 'recording.webm');
      form.append('duration_ms', String(Math.round(durationMs)));
      return api<{ text: string }>(`/workspaces/${ws}/assistant/transcribe`, { method: 'POST', body: form });
    },
  });
}

/** The workspace agent's name (falls back to the default while loading or if not set up). */
export function useAssistantName(ws: string): string {
  const agent = useAgent(ws);
  return agent.data?.agent?.name || ASSISTANT_NAME;
}
