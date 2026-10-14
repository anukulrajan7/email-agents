import type { AgentCategory, AgentList, AgentSummary } from '../api/types';

export function agent(over: Partial<AgentSummary> & Pick<AgentSummary, 'id' | 'name' | 'category'>): AgentSummary {
  return {
    key: over.id,
    kind: 'system',
    scope: 'workspace',
    owner_user_id: null,
    avatar: '🌼',
    color: 'ochre',
    description: `${over.name} role`,
    status: 'active',
    paused_reason: null,
    template_key: null,
    tuned: false,
    update_available: false,
    active_version: 1,
    autonomy: { earned: 1, cap: 3, effective: 1, limited_by: 'earned' },
    today: { routed: 0, cases: 0, approvals_pending: 0, cost_usd: 0 },
    ...over,
  };
}

export function registry(agents: AgentSummary[], me: AgentSummary | null = null, paused_all = false): AgentList {
  const names: Record<AgentCategory, string> = { general: 'General', support: 'Support', sales: 'Sales', finance: 'Finance', personal: 'Personal', hrms: 'HRMS', custom: 'Custom' };
  const keys = Array.from(new Set(agents.map((a) => a.category)));
  return { paused_all, me, categories: keys.map((key) => ({ key, name: names[key], agents: agents.filter((a) => a.category === key) })) };
}

export const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

export const meBody = (role: 'admin' | 'operator' = 'admin', extra: Record<string, unknown> = {}) => ({
  user: { id: 'u1', email: 'a@b.test', name: 'Asha' },
  workspaces: [{ id: 'w1', name: 'Acme', role, created_at: '', ...extra }],
  auth_mode: 'password',
});
