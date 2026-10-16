import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { AiHealth, FlagMode, WorkspaceFlag } from './types';

export const HEALTH_DAYS = 14;

export const healthKey = (ws: string) => ['ws', ws, 'ai-health'] as const;

export function useAiHealth(ws: string, enabled: boolean) {
  return useQuery({
    queryKey: [...healthKey(ws), HEALTH_DAYS],
    queryFn: () => api<AiHealth>(`/workspaces/${ws}/ai-health`, { query: { days: HEALTH_DAYS } }),
    enabled,
  });
}

export function useResolveIncident(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api<{ id: string; resolved: boolean }>(`/workspaces/${ws}/ai-incidents/${id}/resolve`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: healthKey(ws) }),
  });
}

export const flagsKey = (ws: string) => ['ws', ws, 'flags'] as const;

export function useFlags(ws: string, enabled: boolean) {
  return useQuery({
    queryKey: flagsKey(ws),
    queryFn: () => api<{ items: WorkspaceFlag[] }>(`/workspaces/${ws}/flags`),
    enabled,
  });
}

export function useSetFlag(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ key, mode }: { key: string; mode: FlagMode }) => api<WorkspaceFlag>(`/workspaces/${ws}/flags/${key}`, { method: 'PUT', body: { mode } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: flagsKey(ws) }),
  });
}
