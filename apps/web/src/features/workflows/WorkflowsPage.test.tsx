import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { WorkflowsPage } from './WorkflowsPage';

const workflow = {
  id: 'wf1',
  template_key: 'timesheet_review',
  label_key: 'timesheet',
  name: 'Timesheet review',
  description: 'Reviews timesheets',
  enabled: false,
  available: false,
  last_run: null,
};

describe('WorkflowsPage', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('calls the setup endpoint from the add-label button', async () => {
    const fetchMock = vi.fn((url: string, init?: RequestInit) => {
      if (url.endsWith('/me')) {
        return Promise.resolve(
          new Response(JSON.stringify({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role: 'admin', created_at: '' }], auth_mode: 'password' })),
        );
      }
      if (init?.method === 'POST') return Promise.resolve(new Response(JSON.stringify({ ...workflow, available: true, enabled: true })));
      return Promise.resolve(new Response(JSON.stringify({ items: url.includes('/workflows') && !url.includes('runs') ? [workflow] : [] })));
    });
    vi.stubGlobal('fetch', fetchMock);
    localStorage.setItem('auth_token', 'dev:a@b.test');
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter initialEntries={['/w/w1/workflows']}>
          <Routes>
            <Route path="/w/:ws/workflows" element={<WorkflowsPage />} />
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );
    await userEvent.click(await screen.findByRole('button', { name: 'Add “Timesheet” label & enable' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith('/api/v1/workspaces/w1/workflows/wf1/setup', expect.objectContaining({ method: 'POST' })),
    );
  });
});
