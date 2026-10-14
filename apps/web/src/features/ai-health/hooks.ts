import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { AiHealth } from './types';

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
