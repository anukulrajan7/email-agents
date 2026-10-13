import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api, getToken } from '../../api/client';
import type { Items, MeResponse, Workspace } from '../../api/types';

export const meKey = ['me'] as const;

export function useMe() {
  return useQuery({
    queryKey: meKey,
    queryFn: () => api<MeResponse>('/me'),
    enabled: getToken() !== undefined,
    staleTime: 60_000,
  });
}

export function useCreateWorkspace() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => api<Workspace>('/workspaces', { method: 'POST', body: { name } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: meKey }),
  });
}

export function useWorkspaces() {
  return useQuery({ queryKey: ['workspaces'], queryFn: () => api<Items<Workspace>>('/workspaces') });
}
