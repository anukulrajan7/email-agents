import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Candidate, Items, Match, MatchStatus, Requirement } from '../../api/types';

export type MatchFilters = { requirement_id?: string; candidate_id?: string; status?: MatchStatus };

const matchingKey = (ws: string) => ['ws', ws, 'matching'] as const;

export function useRequirements(ws: string, enabled = true) {
  return useQuery({
    queryKey: [...matchingKey(ws), 'requirements'],
    queryFn: () => api<Items<Requirement>>(`/workspaces/${ws}/matching/requirements`),
    enabled,
    refetchInterval: 10000,
  });
}

export function useCandidates(ws: string) {
  return useQuery({
    queryKey: [...matchingKey(ws), 'candidates'],
    queryFn: () => api<Items<Candidate>>(`/workspaces/${ws}/matching/candidates`),
    refetchInterval: 10000,
  });
}

export function useMatches(ws: string, filters: MatchFilters = {}) {
  return useQuery({
    queryKey: [...matchingKey(ws), 'matches', filters],
    queryFn: () => api<Items<Match>>(`/workspaces/${ws}/matching/matches`, { query: filters }),
    refetchInterval: 10000,
  });
}

export function useDecideMatch(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, status }: { id: string; status: MatchStatus }) =>
      api<Match>(`/workspaces/${ws}/matching/matches/${id}`, { method: 'PATCH', body: { status } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: matchingKey(ws) }),
  });
}

export function useCloseRequirement(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) =>
      api<{ id: string; status: 'open' | 'closed' }>(`/workspaces/${ws}/matching/requirements/${id}`, {
        method: 'PATCH',
        body: { status: 'closed' },
      }),
    onSuccess: () => qc.invalidateQueries({ queryKey: matchingKey(ws) }),
  });
}

export function useSetupMatching(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<{ labels: string[] }>(`/workspaces/${ws}/matching/setup`, { method: 'POST' }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: matchingKey(ws) });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'labels'] });
    },
  });
}
