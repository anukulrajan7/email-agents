import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type { Items, NangoConnectionBody, NangoProvider, NangoSession, Source, TestEmailBody } from '../../api/types';

const key = (ws: string) => ['ws', ws, 'sources'] as const;

export function useSources(ws: string, opts: { poll?: boolean } = {}) {
  return useQuery({
    queryKey: key(ws),
    queryFn: () => api<Items<Source>>(`/workspaces/${ws}/sources`),
    refetchInterval: opts.poll ? 5000 : false,
  });
}

export function useCreateForwardingSource(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api<Source>(`/workspaces/${ws}/sources/forwarding`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: key(ws) }),
  });
}

export function useSendTestEmail(ws: string, sourceId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: TestEmailBody) =>
      api<{ status: 'queued' }>(`/workspaces/${ws}/sources/${sourceId}/test-email`, { method: 'POST', body }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['ws', ws, 'messages'] }),
  });
}

export function useSendTestEmailEnsuringSource(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      const { items } = await api<Items<Source>>(`/workspaces/${ws}/sources`);
      const source = items.find((s) => s.kind === 'forwarding') ?? (await api<Source>(`/workspaces/${ws}/sources/forwarding`, { method: 'POST' }));
      return api<{ status: 'queued' }>(`/workspaces/${ws}/sources/${source.id}/test-email`, { method: 'POST', body: {} });
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: key(ws) });
      void qc.invalidateQueries({ queryKey: ['ws', ws, 'messages'] });
    },
  });
}

export function useNangoSession(ws: string) {
  return useMutation({
    mutationFn: (provider: NangoProvider) =>
      api<NangoSession>(`/workspaces/${ws}/sources/nango-session`, { method: 'POST', body: { provider } }),
  });
}

/** Reports the connection Nango Connect created; the server re-verifies it with Nango. */
export function useRegisterNangoConnection(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: NangoConnectionBody) =>
      api<Source>(`/workspaces/${ws}/sources/nango-connections`, { method: 'POST', body }),
    onSuccess: () => qc.invalidateQueries({ queryKey: key(ws) }),
  });
}

export function useSyncSource(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (sourceId: string) =>
      api<{ status: 'queued' }>(`/workspaces/${ws}/sources/${sourceId}/sync`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: key(ws) }),
  });
}

export function useDisconnectSource(ws: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (sourceId: string) => api<Source>(`/workspaces/${ws}/sources/${sourceId}/disconnect`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: key(ws) }),
  });
}
