import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Approval, ApprovalDecision, ApprovalList, CaseDetail, CaseFilter, CaseList, CaseSummary, StartCaseBody } from './types';
import { FILTER_STATUS } from './labels';

export const AGENT_POLL_MS = 30_000;

const agentsKey = (ws: string) => ['ws', ws, 'agents'] as const;

export function useApprovals(ws: string) {
  return useQuery({
    queryKey: [...agentsKey(ws), 'approvals'],
    queryFn: () => api<ApprovalList>(`/workspaces/${ws}/approvals`, { query: { status: 'pending' } }),
    refetchInterval: AGENT_POLL_MS,
  });
}

export function usePendingApprovalCount(ws: string): number {
  return useApprovals(ws).data?.pending_count ?? 0;
}

export function useCases(ws: string, filter: CaseFilter) {
  const status = FILTER_STATUS[filter];
  return useQuery({
    queryKey: [...agentsKey(ws), 'cases', status ?? 'any'],
    queryFn: () => api<CaseList>(`/workspaces/${ws}/cases`, { query: { status } }),
    refetchInterval: AGENT_POLL_MS,
  });
}

export function useCase(ws: string, id: string) {
  return useQuery({
    queryKey: [...agentsKey(ws), 'case', id],
    queryFn: () => api<CaseDetail>(`/workspaces/${ws}/cases/${id}`),
    refetchInterval: AGENT_POLL_MS,
  });
}

export function useDecideApproval(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...body }: ApprovalDecision & { id: string }) =>
      api<Approval>(`/workspaces/${ws}/approvals/${id}`, { method: 'POST', body }),
    onSuccess: () => qc.invalidateQueries({ queryKey: agentsKey(ws) }),
  });
}

export function useStartCase(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: StartCaseBody) => api<CaseSummary>(`/workspaces/${ws}/cases`, { method: 'POST', body }),
    onSuccess: () => qc.invalidateQueries({ queryKey: agentsKey(ws) }),
  });
}

export function useCancelCase(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api<CaseSummary>(`/workspaces/${ws}/cases/${id}/cancel`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: agentsKey(ws) }),
  });
}
