import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Items, Job } from '../../api/types';

const key = (ws: string) => ['ws', ws, 'jobs', 'dead'] as const;

export function useDeadJobs(ws: string, enabled: boolean) {
  return useQuery({
    queryKey: key(ws),
    queryFn: () => api<Items<Job>>(`/workspaces/${ws}/jobs`, { query: { status: 'dead' } }),
    enabled,
  });
}

export function useRetryJob(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api<Job>(`/workspaces/${ws}/jobs/${id}/retry`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: key(ws) }),
  });
}
