import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { AgentMetrics, AutonomyState, EvalList, EvalRun, Scorecard } from './types';

export const METRIC_DAYS = 30;

const metricsKey = (ws: string) => ['ws', ws, 'metrics'] as const;

export function useScorecard(ws: string) {
  return useQuery({
    queryKey: [...metricsKey(ws), 'scorecard'],
    queryFn: () => api<Scorecard>(`/workspaces/${ws}/scorecard`),
  });
}

export function useAgentMetrics(ws: string, agentId: string) {
  return useQuery({
    queryKey: [...metricsKey(ws), 'agent', agentId],
    queryFn: () => api<AgentMetrics>(`/workspaces/${ws}/agents/${agentId}/metrics`, { query: { days: METRIC_DAYS } }),
  });
}

export function useEvals(ws: string, agentId: string) {
  return useQuery({
    queryKey: [...metricsKey(ws), 'evals', agentId],
    queryFn: () => api<EvalList>(`/workspaces/${ws}/agents/${agentId}/evals`),
    refetchInterval: (query) => (query.state.data?.items.some((r) => r.status === 'queued' || r.status === 'running') ? 3_000 : false),
  });
}

export function useStartEval(ws: string, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (suite: string) => api<EvalRun>(`/workspaces/${ws}/agents/${agentId}/evals`, { method: 'POST', body: { suite } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: metricsKey(ws) }),
  });
}

export function useAutonomy(ws: string, agentId: string) {
  return useQuery({
    queryKey: [...metricsKey(ws), 'autonomy', agentId],
    queryFn: () => api<AutonomyState>(`/workspaces/${ws}/agents/${agentId}/autonomy`),
  });
}

export function useChangeLevel(ws: string, agentId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (level: number) => api<AutonomyState>(`/workspaces/${ws}/agents/${agentId}/autonomy`, { method: 'POST', body: { level } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: metricsKey(ws) }),
  });
}
