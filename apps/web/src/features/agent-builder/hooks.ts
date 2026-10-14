import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { DesignedSpec, DesignerSession, SavedAgent } from './types';

const sessionKey = (ws: string, id: string | undefined) => ['ws', ws, 'designer', id] as const;
export const DRY_RUN_POLL_MS = 1500;

export function useDesignerSession(ws: string, id: string | undefined) {
  return useQuery({
    queryKey: sessionKey(ws, id),
    queryFn: () => api<DesignerSession>(`/workspaces/${ws}/designer/sessions/${id}`),
    enabled: id !== undefined,
    refetchInterval: (q) => (q.state.data?.dry_run?.status === 'running' ? DRY_RUN_POLL_MS : false),
  });
}

function useSessionMutation<V>(ws: string, run: (v: V) => Promise<DesignerSession>) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: run,
    onSuccess: (s) => qc.setQueryData(sessionKey(ws, s.id), s),
  });
}

export function useStartDesign(ws: string) {
  return useSessionMutation(ws, (goal: string) =>
    api<DesignerSession>(`/workspaces/${ws}/designer/sessions`, { method: 'POST', body: { goal } }),
  );
}

export function useTellDesigner(ws: string, id: string) {
  return useSessionMutation(ws, (text: string) =>
    api<DesignerSession>(`/workspaces/${ws}/designer/sessions/${id}/messages`, { method: 'POST', body: { text } }),
  );
}

export function useEditSpec(ws: string, id: string) {
  return useSessionMutation(ws, (spec: DesignedSpec) =>
    api<DesignerSession>(`/workspaces/${ws}/designer/sessions/${id}/spec`, { method: 'PUT', body: { spec } }),
  );
}

export function useDryRun(ws: string, id: string) {
  return useSessionMutation(ws, () => api<DesignerSession>(`/workspaces/${ws}/designer/sessions/${id}/dry-run`, { method: 'POST', body: {} }));
}

export function useSaveDesign(ws: string, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { start_level: 0 | 1; confirm_steals: boolean }) =>
      api<SavedAgent>(`/workspaces/${ws}/designer/sessions/${id}/save`, { method: 'POST', body }),
    onSuccess: (saved) => {
      qc.setQueryData(sessionKey(ws, id), saved.session);
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'agents'] });
    },
  });
}
