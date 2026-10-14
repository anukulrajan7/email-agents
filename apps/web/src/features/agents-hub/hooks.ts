import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import { agentKey, agentsKey } from '../agents/hooks';
import type {
  AgentAuditItem,
  AgentDetail,
  AgentList,
  AgentPatch,
  AgentSpec,
  AgentSummary,
  AgentVersionInfo,
  Items,
  OutgoingItem,
} from '../../api/types';

export const registryKey = (ws: string) => ['ws', ws, 'agent-registry'] as const;

export function useAgentRegistry(ws: string) {
  return useQuery({
    queryKey: [...registryKey(ws), 'list'],
    queryFn: () => api<AgentList>(`/workspaces/${ws}/agents`),
  });
}

export function useAgentDetail(ws: string, id: string) {
  return useQuery({
    queryKey: [...registryKey(ws), 'detail', id],
    queryFn: () => api<AgentDetail>(`/workspaces/${ws}/agents/${id}`),
  });
}

export function useAgentVersions(ws: string, id: string) {
  return useQuery({
    queryKey: [...registryKey(ws), 'versions', id],
    queryFn: () => api<Items<AgentVersionInfo>>(`/workspaces/${ws}/agents/${id}/versions`),
  });
}

export function useAgentOutgoing(ws: string, id: string) {
  return useQuery({
    queryKey: [...registryKey(ws), 'outgoing', id],
    queryFn: () => api<Items<OutgoingItem>>(`/workspaces/${ws}/agents/${id}/outgoing`),
    refetchInterval: 15_000,
  });
}

export function useAgentAudit(ws: string, id: string) {
  return useQuery({
    queryKey: [...registryKey(ws), 'audit', id],
    queryFn: () => api<Items<AgentAuditItem>>(`/workspaces/${ws}/agents/${id}/audit`),
  });
}

export function usePatchAgent(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: AgentPatch & { id: string }) =>
      api<AgentSummary>(`/workspaces/${ws}/agents/${id}`, { method: 'PATCH', body }),
    onSuccess: (saved) => {
      qc.setQueriesData<AgentList>({ queryKey: [...registryKey(ws), 'list'] }, (list) => (list ? replaceAgent(list, saved) : list));
      void qc.invalidateQueries({ queryKey: registryKey(ws) });
      void qc.invalidateQueries({ queryKey: agentsKey(ws) });
      void qc.invalidateQueries({ queryKey: agentKey(ws) });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'metrics'] });
    },
  });
}

function replaceAgent(list: AgentList, saved: AgentSummary): AgentList {
  const swap = (a: AgentSummary): AgentSummary => (a.id === saved.id ? { ...a, ...saved } : a);
  return { ...list, me: list.me ? swap(list.me) : list.me, categories: list.categories.map((c) => ({ ...c, agents: c.agents.map(swap) })) };
}

export function usePauseAll(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (paused: boolean) => api<unknown>(`/workspaces/${ws}/agents/pause-all`, { method: 'POST', body: { paused } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: registryKey(ws) }),
  });
}

export function useActivateVersion(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (version: number) => api<unknown>(`/workspaces/${ws}/agents/${id}/versions/${version}/activate`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: registryKey(ws) }),
  });
}

export function useStopSend(ws: string, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (draftId: string) => api<{ status: 'cancelled'; draft_id: string }>(`/workspaces/${ws}/drafts/${draftId}/undo-send`, { method: 'POST' }),
    onSettled: () => qc.invalidateQueries({ queryKey: [...registryKey(ws), 'outgoing', agentId] }),
  });
}

export function useSaveSpec(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (spec: AgentSpec) => api<AgentDetail>(`/workspaces/${ws}/agents/${id}/spec`, { method: 'PUT', body: { spec, note: 'Triggers edited' } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: registryKey(ws) }),
  });
}
