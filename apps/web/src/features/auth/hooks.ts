import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from 'react-router';
import { api, clearToken, getToken, setToken } from '../../api/client';
import type { AuthConfig, AuthSession, Items, LabelTemplate, LoginBody, MeResponse, SignupBody, SignupResponse, VerifyBody, Workspace } from '../../api/types';

export const meKey = ['me'] as const;

export function useMe() {
  return useQuery({
    queryKey: meKey,
    queryFn: () => api<MeResponse>('/me'),
    enabled: getToken() !== undefined,
    staleTime: 60_000,
  });
}

export function useCreateWorkspace() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: { name: string; template: LabelTemplate }) => api<Workspace>('/workspaces', { method: 'POST', body }),
    onSuccess: () => qc.invalidateQueries({ queryKey: meKey }),
  });
}

export function useWorkspaces() {
  return useQuery({ queryKey: ['workspaces'], queryFn: () => api<Items<Workspace>>('/workspaces') });
}

export function useAuthConfig() {
  return useQuery({
    queryKey: ['auth-config'],
    queryFn: () => api<AuthConfig>('/auth/config'),
    staleTime: 5 * 60_000,
  });
}

function useStartSession() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  return (session: AuthSession) => {
    setToken(session.token);
    qc.clear();
    navigate('/workspaces');
  };
}

export function useLogin() {
  const start = useStartSession();
  return useMutation({
    mutationFn: (body: LoginBody) => api<AuthSession>('/auth/login', { method: 'POST', body }),
    onSuccess: start,
  });
}

export function useSignup() {
  const start = useStartSession();
  return useMutation({
    mutationFn: (body: SignupBody) => api<SignupResponse>('/auth/signup', { method: 'POST', body }),
    onSuccess: (res) => {
      if ('token' in res) start(res);
    },
  });
}

export function useVerify() {
  const start = useStartSession();
  return useMutation({
    mutationFn: (body: VerifyBody) => api<AuthSession>('/auth/verify', { method: 'POST', body }),
    onSuccess: start,
  });
}

export function useResendCode() {
  return useMutation({
    mutationFn: (email: string) => api<{ status: 'sent' }>('/auth/resend-code', { method: 'POST', body: { email } }),
  });
}

export function useLogout() {
  const qc = useQueryClient();
  const navigate = useNavigate();
  return useMutation({
    mutationFn: async () => {
      try {
        await api<void>('/auth/logout', { method: 'POST' });
      } catch {
        return;
      }
    },
    onSettled: () => {
      clearToken();
      qc.clear();
      navigate('/sign-in');
    },
  });
}
