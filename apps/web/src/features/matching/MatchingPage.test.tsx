import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MatchingPage } from './MatchingPage';

const requirement = {
  id: 'r1', message_id: 'm1', client: 'Acme', role_title: 'Senior React Developer', must_have_skills: ['React'],
  nice_to_have_skills: [], min_experience: 5, max_experience: 8, location: 'Hyderabad', positions: 1, status: 'open',
  source: 'llm', created_at: '', match_count: 1, top_score: 80,
};
const match = {
  id: 'x1', score: 80, status: 'suggested',
  reasons: [{ ok: true, text: 'Knows React' }, { ok: false, text: 'Missing GraphQL' }],
  requirement: { id: 'r1', role_title: 'Senior React Developer', client: 'Acme', location: 'Hyderabad' },
  candidate: { id: 'c1', name: 'Asha Rao', total_experience: 6, location: 'Hyderabad', notice_days: 30 },
  created_at: '', updated_at: '',
};

function setup(role: 'admin' | 'operator', data: { requirements: unknown[]; candidates: unknown[]; matches: unknown[] }) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    if (url.endsWith('/me')) {
      return Promise.resolve(
        new Response(JSON.stringify({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role, created_at: '' }], auth_mode: 'password' })),
      );
    }
    if (init?.method === 'PATCH') return Promise.resolve(new Response(JSON.stringify({ ...match, status: 'shortlisted' })));
    if (init?.method === 'POST') return Promise.resolve(new Response(JSON.stringify({ labels: [] })));
    const key = url.includes('/requirements') ? 'requirements' : url.includes('/candidates') ? 'candidates' : 'matches';
    return Promise.resolve(new Response(JSON.stringify({ items: data[key] })));
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/matching?req=r1']}>
        <Routes>
          <Route path="/w/:ws/matching" element={<MatchingPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('MatchingPage', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('shortlists a match with PATCH', async () => {
    const fetchMock = setup('operator', { requirements: [requirement], candidates: [], matches: [match] });
    await userEvent.click(await screen.findByRole('button', { name: 'Shortlist Asha Rao' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/matching/matches/x1',
        expect.objectContaining({ method: 'PATCH', body: JSON.stringify({ status: 'shortlisted' }) }),
      ),
    );
  });

  it('renders reasons with accessible text and the score', async () => {
    setup('operator', { requirements: [requirement], candidates: [], matches: [match] });
    expect(await screen.findByRole('img', { name: 'Score 80 of 100' })).toBeInTheDocument();
    const list = screen.getByRole('list', { name: 'Why this match' });
    expect(list).toHaveTextContent('✓');
    expect(list).toHaveTextContent('Met: Knows React');
    expect(list).toHaveTextContent('✕');
    expect(list).toHaveTextContent('Not met: Missing GraphQL');
  });

  it('shows the setup button to admins on an empty workspace', async () => {
    const fetchMock = setup('admin', { requirements: [], candidates: [], matches: [] });
    await userEvent.click(await screen.findByRole('button', { name: 'Set up matching labels' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith('/api/v1/workspaces/w1/matching/setup', expect.objectContaining({ method: 'POST' })),
    );
    expect(await screen.findByText(/Labels ready/)).toBeInTheDocument();
  });
});
