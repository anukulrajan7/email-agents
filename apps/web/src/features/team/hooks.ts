import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Escalations, ImportReport, MyMetrics, PeopleList, Person, PersonInput, TeamAccess, TeamMetrics, TeamSettings } from './types';

export const METRICS_POLL_MS = 60_000;

const teamKey = (ws: string) => ['ws', ws, 'team'] as const;

export function useTeamAccess(ws: string, enabled = true) {
  return useQuery({ queryKey: [...teamKey(ws), 'access'], queryFn: () => api<TeamAccess>(`/workspaces/${ws}/team/access`), enabled, staleTime: 60_000 });
}

export function useMyMetrics(ws: string) {
  return useQuery({
    queryKey: [...teamKey(ws), 'metrics', 'me'],
    queryFn: () => api<MyMetrics>(`/workspaces/${ws}/team/metrics/me`),
    refetchInterval: METRICS_POLL_MS,
  });
}

export function useTeamMetrics(ws: string) {
  return useQuery({
    queryKey: [...teamKey(ws), 'metrics', 'team'],
    queryFn: () => api<TeamMetrics>(`/workspaces/${ws}/team/metrics/team`),
    refetchInterval: METRICS_POLL_MS,
  });
}

export function usePeople(ws: string) {
  return useQuery({ queryKey: [...teamKey(ws), 'people'], queryFn: () => api<PeopleList>(`/workspaces/${ws}/team/people`) });
}

function useInvalidateTeam(ws: string) {
  const qc = useQueryClient();
  return () => qc.invalidateQueries({ queryKey: teamKey(ws) });
}

export function useSavePerson(ws: string) {
  const invalidate = useInvalidateTeam(ws);
  return useMutation({
    mutationFn: ({ id, body }: { id?: string; body: Partial<PersonInput> }) =>
      id
        ? api<Person>(`/workspaces/${ws}/team/people/${id}`, { method: 'PATCH', body })
        : api<Person>(`/workspaces/${ws}/team/people`, { method: 'POST', body }),
    onSuccess: invalidate,
  });
}

export function useArchivePerson(ws: string) {
  const invalidate = useInvalidateTeam(ws);
  return useMutation({
    mutationFn: (id: string) => api<void>(`/workspaces/${ws}/team/people/${id}`, { method: 'DELETE' }),
    onSuccess: invalidate,
  });
}

export function useImportPeople(ws: string) {
  const invalidate = useInvalidateTeam(ws);
  return useMutation({
    mutationFn: ({ csv, dry_run }: { csv: string; dry_run: boolean }) =>
      api<ImportReport>(`/workspaces/${ws}/team/people/import`, { method: 'POST', body: { csv, dry_run } }),
    onSuccess: (_report, vars) => (vars.dry_run ? undefined : invalidate()),
  });
}

export function useEscalations(ws: string) {
  return useQuery({ queryKey: [...teamKey(ws), 'escalations'], queryFn: () => api<Escalations>(`/workspaces/${ws}/team/escalations`) });
}

export function useTeamSettings(ws: string) {
  return useQuery({ queryKey: [...teamKey(ws), 'settings'], queryFn: () => api<TeamSettings>(`/workspaces/${ws}/team/settings`) });
}

export function useSaveTeamSettings(ws: string) {
  const invalidate = useInvalidateTeam(ws);
  return useMutation({
    mutationFn: (body: TeamSettings) => api<TeamSettings>(`/workspaces/${ws}/team/settings`, { method: 'PUT', body }),
    onSuccess: invalidate,
  });
}
