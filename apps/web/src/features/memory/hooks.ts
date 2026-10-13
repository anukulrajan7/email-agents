import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from '../../api/client';
import type {
  Contact,
  ContactDetail,
  Items,
  Memory,
  MemoryCreateBody,
  MemoryScope,
  MemoryStatus,
} from '../../api/types';
import type { LearnedMemory, MemoryUpdate } from './types';

export const memoriesKey = (ws: string) => ['ws', ws, 'memories'] as const;
export const contactsKey = (ws: string) => ['ws', ws, 'contacts'] as const;

export type MemoryFilters = { status?: MemoryStatus; scope?: MemoryScope; contact?: string; q?: string };

export function useMemories(ws: string, filters: MemoryFilters, enabled = true) {
  return useQuery({
    queryKey: [...memoriesKey(ws), filters],
    queryFn: () => api<Items<LearnedMemory>>(`/workspaces/${ws}/memories`, { query: filters }),
    enabled,
  });
}

function useInvalidateMemory(ws: string) {
  const qc = useQueryClient();
  return () => {
    void qc.invalidateQueries({ queryKey: memoriesKey(ws) });
    void qc.invalidateQueries({ queryKey: contactsKey(ws) });
    void qc.invalidateQueries({ queryKey: ['ws', ws, 'message'] });
  };
}

export function useCreateMemory(ws: string) {
  const invalidate = useInvalidateMemory(ws);
  return useMutation({
    mutationFn: (body: MemoryCreateBody) => api<Memory>(`/workspaces/${ws}/memories`, { method: 'POST', body }),
    onSuccess: invalidate,
  });
}

export function useUpdateMemory(ws: string) {
  const invalidate = useInvalidateMemory(ws);
  return useMutation({
    mutationFn: ({ id, ...patch }: MemoryUpdate & { id: string }) =>
      api<Memory>(`/workspaces/${ws}/memories/${id}`, { method: 'PATCH', body: patch }),
    onSuccess: invalidate,
  });
}

export function useContacts(ws: string, q: string) {
  return useQuery({
    queryKey: [...contactsKey(ws), 'list', q],
    queryFn: () => api<Items<Contact>>(`/workspaces/${ws}/contacts`, { query: { q } }),
  });
}

export function useContact(ws: string, email: string) {
  return useQuery({
    queryKey: [...contactsKey(ws), 'detail', email],
    queryFn: () => api<ContactDetail>(`/workspaces/${ws}/contacts/${encodeURIComponent(email)}`),
  });
}
