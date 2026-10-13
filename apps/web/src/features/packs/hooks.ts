import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import { meKey } from '../auth/hooks';
import type { Items, Pack, WorkspacePacks } from '../../api/types';

export function usePackCatalogue() {
  return useQuery({
    queryKey: ['packs'],
    queryFn: () => api<Items<Pack>>('/packs'),
    staleTime: 5 * 60_000,
  });
}

export const packsKey = (ws: string) => ['ws', ws, 'packs'] as const;

export function useWorkspacePacks(ws: string) {
  return useQuery({ queryKey: packsKey(ws), queryFn: () => api<WorkspacePacks>(`/workspaces/${ws}/packs`) });
}

export function useSetPacks(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (packs: string[]) => api<WorkspacePacks>(`/workspaces/${ws}/packs`, { method: 'PUT', body: { packs } }),
    onSuccess: (data) => {
      qc.setQueryData(packsKey(ws), data);
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'labels'] });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'workflows'] });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'automations'] });
      void qc.invalidateQueries({ queryKey: meKey });
    },
  });
}
