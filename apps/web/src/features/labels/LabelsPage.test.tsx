import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Label } from '../../api/types';
import { LabelsPage } from './LabelsPage';

const lab = (over: Partial<Label>): Label => ({
  id: 'l1',
  key: 'timesheet',
  name: 'Timesheet',
  description: 'Weekly timesheets from staff',
  color: 'slate',
  position: 1,
  system: false,
  archived: false,
  message_count: 2,
  ...over,
});

const labels = [lab({}), lab({ id: 'l2', key: 'other', name: 'Other', description: 'Anything that fits nowhere else', color: 'rule', position: 2, system: true })];

function setup() {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    if (url.endsWith('/me')) {
      return Promise.resolve(
        new Response(JSON.stringify({ user: { id: 'u', email: 'a@b.test', name: null }, workspaces: [{ id: 'w1', name: 'W', role: 'admin', created_at: '' }], auth_mode: 'password' })),
      );
    }
    if (init?.method === 'POST') return Promise.resolve(new Response(JSON.stringify(lab({ id: 'l3' })), { status: 201 }));
    return Promise.resolve(new Response(JSON.stringify({ items: labels })));
  });
  vi.stubGlobal('fetch', fetchMock);
  localStorage.setItem('auth_token', 'dev:a@b.test');
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter initialEntries={['/w/w1/labels']}>
        <Routes>
          <Route path="/w/:ws/labels" element={<LabelsPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return fetchMock;
}

describe('LabelsPage', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    localStorage.clear();
  });

  it('disables archiving the system label', async () => {
    setup();
    expect(await screen.findByRole('button', { name: 'Archive Other' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Archive Timesheet' })).toBeEnabled();
  });

  it('creates a label with name, description and colour', async () => {
    const fetchMock = setup();
    const user = userEvent.setup();
    const form = (await screen.findByRole('heading', { name: 'Add a label' })).parentElement as HTMLElement;
    await user.type(within(form).getByLabelText('Name'), 'Invoices');
    await user.type(within(form).getByLabelText('Description'), 'Invoices and receipts from vendors');
    await user.click(within(form).getByRole('radio', { name: 'plum' }));
    await user.click(within(form).getByRole('button', { name: 'Add label' }));
    await waitFor(() => {
      const call = fetchMock.mock.calls.find(([, init]) => init?.method === 'POST');
      expect(call?.[0]).toContain('/workspaces/w1/labels');
      expect(JSON.parse(call?.[1]?.body as string)).toEqual({
        name: 'Invoices',
        description: 'Invoices and receipts from vendors',
        color: 'plum',
      });
    });
  });
});
