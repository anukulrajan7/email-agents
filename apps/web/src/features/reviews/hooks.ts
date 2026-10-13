import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Items, ReviewItem, ReviewStatus } from '../../api/types';

export type ReviewFilter = ReviewStatus | 'all';

export function useReviews(ws: string, status: ReviewFilter) {
  return useQuery({
    queryKey: ['ws', ws, 'reviews', status],
    queryFn: () => api<Items<ReviewItem>>(`/workspaces/${ws}/reviews`, { query: { status: status === 'all' ? undefined : status } }),
    refetchInterval: 10_000,
  });
}

export function useSetReviewStatus(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, status }: { id: string; status: ReviewStatus }) =>
      api<ReviewItem>(`/workspaces/${ws}/reviews/${id}`, { method: 'PATCH', body: { status } }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'reviews'] });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'message'] });
    },
  });
}
