import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Items, Workflow, WorkflowRun } from '../../api/types';

export function useWorkflows(ws: string) {
  return useQuery({ queryKey: ['ws', ws, 'workflows'], queryFn: () => api<Items<Workflow>>(`/workspaces/${ws}/workflows`) });
}

export function useWorkflowRuns(ws: string) {
  return useQuery({
    queryKey: ['ws', ws, 'workflow-runs'],
    queryFn: () => api<Items<WorkflowRun>>(`/workspaces/${ws}/workflow-runs`),
    refetchInterval: 5000,
  });
}

export function useToggleWorkflow(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      api<Workflow>(`/workspaces/${ws}/workflows/${id}`, { method: 'PATCH', body: { enabled } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['ws', ws, 'workflows'] }),
  });
}
