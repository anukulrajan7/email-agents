import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Alert, AlertFilter, AlertList, AlertPatch, AlertSettings, AlertTestResult } from './types';

export const ALERT_POLL_MS = 60_000;

export const alertsKey = (ws: string) => ['ws', ws, 'alerts'] as const;
export const alertSettingsKey = (ws: string) => ['ws', ws, 'alert-settings'] as const;

export const DEFAULT_ALERT_SETTINGS: AlertSettings = {
  enabled: true,
  recipients: [],
  escalate_to: [],
  unanswered_hours: 4,
  escalate_after_hours: 24,
  daily_digest: true,
  digest_hour: 9,
};

export function withSettingDefaults(raw: Partial<AlertSettings> | undefined): AlertSettings {
  return { ...DEFAULT_ALERT_SETTINGS, ...raw };
}

export function useAlerts(ws: string, status: AlertFilter) {
  return useQuery({
    queryKey: [...alertsKey(ws), status],
    queryFn: () => api<AlertList>(`/workspaces/${ws}/alerts`, { query: { status } }),
    refetchInterval: ALERT_POLL_MS,
  });
}

export function useOpenAlertCount(ws: string): number {
  const { data } = useAlerts(ws, 'open');
  return data?.open_count ?? data?.items.length ?? 0;
}

export function useUpdateAlert(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ id, ...patch }: AlertPatch & { id: string }) =>
      api<Alert>(`/workspaces/${ws}/alerts/${id}`, { method: 'PATCH', body: patch }),
    onSuccess: () => qc.invalidateQueries({ queryKey: alertsKey(ws) }),
  });
}

export function useAlertSettings(ws: string, enabled = true) {
  return useQuery({
    queryKey: alertSettingsKey(ws),
    queryFn: () => api<Partial<AlertSettings>>(`/workspaces/${ws}/alert-settings`).then(withSettingDefaults),
    enabled,
  });
}

export function useSaveAlertSettings(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: AlertSettings) => api<Partial<AlertSettings>>(`/workspaces/${ws}/alert-settings`, { method: 'PUT', body }),
    onSuccess: () => qc.invalidateQueries({ queryKey: alertSettingsKey(ws) }),
  });
}

export function useSendTestAlert(ws: string) {
  return useMutation({
    mutationFn: () => api<AlertTestResult>(`/workspaces/${ws}/alert-settings/test`, { method: 'POST' }),
  });
}
