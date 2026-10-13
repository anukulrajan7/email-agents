import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { ImportReport } from '../team/types';
import type { MismatchList, Submission, SubmissionList, TimesheetsView } from './types';

const hrmsKey = (ws: string) => ['ws', ws, 'hrms'] as const;

export function useSubmissions(ws: string, linkStatus?: string) {
  return useQuery({
    queryKey: [...hrmsKey(ws), 'submissions', linkStatus ?? 'all'],
    queryFn: () => api<SubmissionList>(`/workspaces/${ws}/staffing/submissions`, { query: { link_status: linkStatus } }),
  });
}

export function useDecideSubmission(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, body }: { id: string; body: { candidate_id?: string | null; requirement_id?: string | null; dismissed?: boolean; outcome?: string } }) =>
      api<Submission>(`/workspaces/${ws}/staffing/submissions/${id}`, { method: 'PATCH', body }),
    onSuccess: () => qc.invalidateQueries({ queryKey: [...hrmsKey(ws), 'submissions'] }),
  });
}

export function useTimesheets(ws: string, month?: string) {
  return useQuery({
    queryKey: [...hrmsKey(ws), 'timesheets', month ?? 'last'],
    queryFn: () => api<TimesheetsView>(`/workspaces/${ws}/staffing/timesheets`, { query: { month } }),
  });
}

export function useMismatches(ws: string) {
  return useQuery({ queryKey: [...hrmsKey(ws), 'mismatches'], queryFn: () => api<MismatchList>(`/workspaces/${ws}/staffing/portal/mismatches`) });
}

export function useImportPortal(ws: string, kind: 'hours' | 'submissions') {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ csv, dry_run }: { csv: string; dry_run: boolean }) =>
      api<ImportReport>(`/workspaces/${ws}/staffing/portal/${kind}`, { method: 'POST', body: { csv, dry_run } }),
    onSuccess: (_r, vars) => (vars.dry_run ? undefined : qc.invalidateQueries({ queryKey: hrmsKey(ws) })),
  });
}

export function useReconcile(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<{ alerts_raised: number; matched: number; mismatches: unknown[] }>(`/workspaces/${ws}/staffing/portal/reconcile`, { method: 'POST' }),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: hrmsKey(ws) });
      qc.invalidateQueries({ queryKey: ['ws', ws, 'alerts'] });
    },
  });
}
