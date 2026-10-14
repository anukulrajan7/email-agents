import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Items, MessageSummary, Paged } from '../../api/types';
import type {
  AgentInput,
  AgentView,
  Approval,
  ApprovalDecision,
  ApprovalList,
  CaseDetail,
  CaseFilter,
  CaseList,
  CaseSummary,
  MyProfileView,
  PreviewBody,
  PreviewResult,
  StartCaseBody,
  TeamProfile,
  UserProfile,
  UserProfileInput,
  WorkspaceAgent,
} from './types';
import { FILTER_STATUS } from './labels';

export const AGENT_POLL_MS = 30_000;

export const agentsKey = (ws: string) => ['ws', ws, 'agents'] as const;

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

export const agentKey = (ws: string) => ['ws', ws, 'agent'] as const;
const myProfileKey = (ws: string) => ['ws', ws, 'my-agent-profile'] as const;

export function useAgent(ws: string) {
  return useQuery({
    queryKey: agentKey(ws),
    queryFn: () => api<AgentView>(`/workspaces/${ws}/agent`),
    staleTime: 30_000,
  });
}

export function useSaveAgent(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: AgentInput) => api<WorkspaceAgent>(`/workspaces/${ws}/agent`, { method: 'PUT', body }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: agentKey(ws) });
      void qc.invalidateQueries({ queryKey: myProfileKey(ws) });
      void qc.invalidateQueries({ queryKey: agentsKey(ws) });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'agent-registry'] });
    },
  });
}

export function useMyProfile(ws: string) {
  return useQuery({
    queryKey: myProfileKey(ws),
    queryFn: () => api<MyProfileView>(`/workspaces/${ws}/me/agent-profile`),
    staleTime: 60_000,
  });
}

export function useSaveMyProfile(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: Partial<UserProfileInput>) => api<UserProfile>(`/workspaces/${ws}/me/agent-profile`, { method: 'PUT', body }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: myProfileKey(ws) });
      void qc.invalidateQueries({ queryKey: [...agentKey(ws), 'team'] });
    },
  });
}

export function useSkipMyProfile(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<{ skipped: boolean }>(`/workspaces/${ws}/me/agent-profile/skip`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: myProfileKey(ws) }),
  });
}

export function useTeamProfiles(ws: string, enabled: boolean) {
  return useQuery({
    queryKey: [...agentKey(ws), 'team'],
    queryFn: () => api<Items<TeamProfile>>(`/workspaces/${ws}/agent/profiles`),
    enabled,
  });
}

export function useRecentEmails(ws: string, enabled: boolean) {
  return useQuery({
    queryKey: ['ws', ws, 'messages', 'agent-test'],
    queryFn: () => api<Paged<MessageSummary>>(`/workspaces/${ws}/messages`, { query: { limit: 15, sort: 'recent' } }),
    enabled,
    staleTime: 30_000,
  });
}

export function usePreviewAgent(ws: string) {
  return useMutation({
    mutationFn: (body: PreviewBody) => api<PreviewResult>(`/workspaces/${ws}/agent/preview`, { method: 'POST', body }),
  });
}
