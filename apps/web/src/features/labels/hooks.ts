import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Items, Label, LabelCreateBody, LabelPatch, LabelTemplateInfo } from '../../api/types';

export const labelsKey = (ws: string) => ['ws', ws, 'labels'] as const;

export function useLabels(ws: string, opts: { includeArchived?: boolean } = {}) {
  const includeArchived = opts.includeArchived ?? false;
  return useQuery({
    queryKey: [...labelsKey(ws), { includeArchived }],
    queryFn: () =>
      api<Items<Label>>(`/workspaces/${ws}/labels`, { query: { include_archived: includeArchived ? 'true' : undefined } }),
    staleTime: 30_000,
  });
}

export function useLabelMap(ws: string) {
  const labels = useLabels(ws, { includeArchived: true });
  const map = new Map<string, Label>();
  for (const l of labels.data?.items ?? []) map.set(l.key, l);
  return map;
}

function useInvalidateLabels(ws: string) {
  const qc = useQueryClient();
  return () => {
    void qc.invalidateQueries({ queryKey: labelsKey(ws) });
    void qc.invalidateQueries({ queryKey: ['ws', ws, 'workflows'] });
  };
}

export function useCreateLabel(ws: string) {
  const invalidate = useInvalidateLabels(ws);
  return useMutation({
    mutationFn: (body: LabelCreateBody) => api<Label>(`/workspaces/${ws}/labels`, { method: 'POST', body }),
    onSuccess: invalidate,
  });
}

export function useUpdateLabel(ws: string) {
  const invalidate = useInvalidateLabels(ws);
  return useMutation({
    mutationFn: ({ id, ...patch }: LabelPatch & { id: string }) =>
      api<Label>(`/workspaces/${ws}/labels/${id}`, { method: 'PATCH', body: patch }),
    onSuccess: invalidate,
  });
}

export type LabelSuggestion = {
  key: string;
  name: string;
  description: string;
  color: string;
  example_subjects: string[];
  estimated_count: number;
};

export function useSuggestLabels(ws: string) {
  return useMutation({
    mutationFn: () => api<Items<LabelSuggestion>>(`/workspaces/${ws}/labels/suggest`, { method: 'POST' }),
  });
}

export function useLabelTemplates() {
  return useQuery({
    queryKey: ['label-templates'],
    queryFn: () => api<Items<LabelTemplateInfo>>('/label-templates'),
    staleTime: 5 * 60_000,
  });
}
