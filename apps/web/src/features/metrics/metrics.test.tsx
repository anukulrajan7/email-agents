import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { distance, millis, money, percent } from './format';
import { AgentMetricsView } from './AgentMetricsView';
import { MetricsRoute } from './MetricsRoute';
import type { AgentIdentity, AutonomyState, EvalList, Totals } from './types';

const agent: AgentIdentity = { id: 'a1', key: 'meera', kind: 'platform', scope: 'workspace', category: 'inbox', name: 'Meera', avatar: 'M', color: 'plum', status: 'active', active_version: 1 };

const totals: Totals = {
  routed: 40,
  cases: 12,
  success_rate: 0.9,
  approval_rate: 0.8,
  decline_rate: 0.2,
  approvals_decided: 10,
  edit_distance_median: 0.12,
  auto_sends: 0,
  stop_rate: null,
  label_accuracy: null,
  shadow_agreement: null,
  cost_usd: 0.4,
  cost_per_email_usd: 0.01,
  cost_per_case_usd: 0.033,
  latency_p50_ms: 1200,
  latency_p95_ms: 3400,
  incidents: 0,
};

const evals: EvalList = {
  items: [
    {
      id: 'r1', suite: 'safety', trigger: 'version', status: 'passed', score: 1, cases_total: 20, cases_passed: 20, baseline_score: 0.95,
      agent_version: 1, cost_usd: 0, note: null, created_at: '2026-10-10T00:00:00Z', finished_at: '2026-10-10T00:01:00Z',
    },
  ],
  versions: [],
  live_available: false,
};

const autonomy: AutonomyState = {
  earned: 1, cap: 3, effective: 1, limited_by: 'earned', level_name: 'Draft for approval',
  why: ['Level 1 (Draft for approval) is the highest this agent has earned.', 'Not ready for level 2 yet: Approval rate.'],
  version: { number: 1, gate_status: 'passed' },
  next: { level: 2, name: 'Send routine replies', requirements: [{ name: 'Approval rate', target: '90% or more', actual: '80%', met: false }] },
  history: [],
};

const json = (body: unknown) => Promise.resolve(new Response(JSON.stringify(body), { status: 200 }));

function setup(path: string, role: 'admin' | 'operator' = 'admin') {
  vi.stubGlobal(
    'fetch',
    vi.fn((url: string) => {
      if (url.endsWith('/me')) {
        return json({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role, created_at: '' }], auth_mode: 'dev' });
      }
      if (url.includes('/scorecard')) {
        return json({ days: 30, totals, agents: [{ agent, totals, autonomy: { earned: 1, cap: 3, ready_for: null }, safety: { status: 'passed', score: 1, created_at: '' } }] });
      }
      if (url.includes('/metrics')) return json({ days: 30, agent, totals, series: [{ date: '2026-10-13', approval_rate: 0.8, edit_distance: null, cost_usd: 0.1, cases: 3 }] });
      if (url.includes('/evals')) return json(evals);
      if (url.includes('/autonomy')) return json(autonomy);
      return json({ items: [] });
    }),
  );
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/w/:ws/metrics" element={<MetricsRoute />} />
          <Route path="/w/:ws/metrics/:agentId" element={<AgentMetricsView />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe('format', () => {
  it('formats missing values as a dash', () => {
    expect(percent(null)).toBe('-');
    expect(percent(0.876)).toBe('88%');
    expect(distance(0.125)).toBe('0.13');
    expect(millis(1200)).toBe('1.2 s');
    expect(millis(80)).toBe('80 ms');
    expect(money(0.0023)).toBe('$0.0023');
  });
});

describe('metrics pages', () => {
  it('lists each agent with its level and safety state', async () => {
    setup('/w/w1/metrics');
    expect(await screen.findByRole('link', { name: /Meera/ })).toHaveAttribute('href', '/w/w1/metrics/a1');
    expect(screen.getByText('Draft for approval')).toBeInTheDocument();
    expect(screen.getByText('Passing')).toBeInTheDocument();
  });

  it('shows check history and the reasons behind the level', async () => {
    const user = userEvent.setup();
    setup('/w/w1/metrics/a1');
    await user.click(await screen.findByRole('tab', { name: 'Checks' }));
    expect(await screen.findByText('Safety checks', { selector: 'span' })).toBeInTheDocument();
    expect(screen.getByText('+5 points')).toBeInTheDocument();
    await user.click(screen.getByRole('tab', { name: 'Why this level' }));
    expect(await screen.findByText(/Not ready for level 2 yet/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Promote to level 2' })).toBeDisabled();
  });

  it('hides actions from people who are not admins', async () => {
    const user = userEvent.setup();
    setup('/w/w1/metrics/a1', 'operator');
    await user.click(await screen.findByRole('tab', { name: 'Checks' }));
    await waitFor(() => expect(screen.getByText('Check history, newest first')).toBeInTheDocument());
    expect(screen.queryByRole('button', { name: 'Run safety checks' })).not.toBeInTheDocument();
  });
});
