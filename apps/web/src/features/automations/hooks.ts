import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Items } from '../../api/types';
import type { Automation, AutomationInput, AutomationRun, Preset, TestResult } from './types';

export const automationsKey = (ws: string) => ['ws', ws, 'automations'] as const;

export function useAutomations(ws: string) {
  return useQuery({
    queryKey: automationsKey(ws),
    queryFn: () => api<Items<Automation>>(`/workspaces/${ws}/automations`),
  });
}

export function usePresets(ws: string) {
  return useQuery({
    queryKey: ['ws', ws, 'automation-presets'],
    queryFn: () => api<Items<Preset>>(`/workspaces/${ws}/automations/presets`),
    staleTime: 5 * 60_000,
  });
}

function useInvalidateAutomations(ws: string) {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: automationsKey(ws) });
}

export function useCreateAutomation(ws: string) {
  const invalidate = useInvalidateAutomations(ws);
  return useMutation({
    mutationFn: (body: AutomationInput) => api<Automation>(`/workspaces/${ws}/automations`, { method: 'POST', body }),
    onSuccess: invalidate,
  });
}

export function useUpdateAutomation(ws: string) {
  const invalidate = useInvalidateAutomations(ws);
  return useMutation({
    mutationFn: ({ id, ...patch }: Partial<AutomationInput> & { id: string }) =>
      api<Automation>(`/workspaces/${ws}/automations/${id}`, { method: 'PATCH', body: patch }),
    onSuccess: invalidate,
  });
}

export function useDeleteAutomation(ws: string) {
  const invalidate = useInvalidateAutomations(ws);
  return useMutation({
    mutationFn: (id: string) => api<void>(`/workspaces/${ws}/automations/${id}`, { method: 'DELETE' }),
    onSuccess: invalidate,
  });
}

export function useCreateFromPreset(ws: string) {
  const invalidate = useInvalidateAutomations(ws);
  return useMutation({
    mutationFn: (key: string) => api<Automation>(`/workspaces/${ws}/automations/presets/${key}`, { method: 'POST' }),
    onSuccess: invalidate,
  });
}

export function useAutomationRuns(ws: string, id: string) {
  return useQuery({
    queryKey: [...automationsKey(ws), id, 'runs'],
    queryFn: () => api<Items<AutomationRun>>(`/workspaces/${ws}/automations/${id}/runs`, { query: { limit: 50 } }),
    refetchInterval: 10_000,
  });
}

export function useTestAutomation(ws: string) {
  return useMutation({
    mutationFn: (id: string) => api<TestResult>(`/workspaces/${ws}/automations/${id}/test`, { method: 'POST' }),
  });
}
