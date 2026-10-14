import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { PacksPage } from './PacksPage';

const pack = (key: string, name: string, enabled: boolean) => ({ key, name, description: `${name} desc`, labels: [{ key: 'other', name: 'Other', description: '' }], presets: [], features: [], enabled });

function mockFetch(role: 'admin' | 'operator') {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    if (url.endsWith('/me')) {
      return Promise.resolve(
        new Response(JSON.stringify({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role, created_at: '' }], auth_mode: 'password' })),
      );
    }
    if (url.includes('dry_run=1')) {
      return Promise.resolve(
        new Response(JSON.stringify({ labels_added: [{ key: 'timesheet', name: 'Timesheet' }, { key: 'leave', name: 'Leave' }], labels_restored: [{ key: 'payroll', name: 'Payroll' }], labels_hidden: [{ key: 'fyi', name: 'FYI' }] })),
      );
    }
    const enabled = init?.method === 'PUT' ? ['general', 'staffing'] : ['general'];
    return Promise.resolve(
      new Response(JSON.stringify({ enabled, items: [pack('general', 'Business inbox', true), pack('staffing', 'HR & recruiting', enabled.includes('staffing'))] })),
    );
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  return fetchMock;
}

function renderPage() {
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/packs']}>
        <Routes>
          <Route path="/w/:ws/packs" element={<PacksPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
}

describe('PacksPage', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('lets an admin turn the staffing pack on', async () => {
    const fetchMock = mockFetch('admin');
    renderPage();
    const staffing = await screen.findByRole('checkbox', { name: /HR & recruiting/ });
    expect(staffing).not.toBeChecked();
    await userEvent.click(staffing);
    expect(await screen.findByText(/This will add 3 and hide 1 labels/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Save packs' }));
    await waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        '/api/v1/workspaces/w1/packs',
        expect.objectContaining({ method: 'PUT', body: JSON.stringify({ packs: ['general', 'staffing'] }) }),
      ),
    );
  });

  it('shows operators a read-only notice', async () => {
    mockFetch('operator');
    renderPage();
    expect(await screen.findByText('Admins only')).toBeInTheDocument();
  });
});
