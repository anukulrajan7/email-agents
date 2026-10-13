import { useQuery } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { AiUsage } from './types';

export const USAGE_DAYS = 30;

export const usageKey = (ws: string) => ['ws', ws, 'ai-usage'] as const;

export function useAiUsage(ws: string, enabled: boolean) {
  return useQuery({
    queryKey: [...usageKey(ws), USAGE_DAYS],
    queryFn: () => api<AiUsage>(`/workspaces/${ws}/ai-usage`, { query: { days: USAGE_DAYS } }),
    enabled,
  });
}
