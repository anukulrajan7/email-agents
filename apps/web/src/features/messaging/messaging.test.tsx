import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ToastProvider } from '../../components/ui';
import { MessagesPage } from './MessagesPage';

const json = (body: unknown) => new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });

const inbox = {
  unread: 1,
  items: [
    { id: 'imsg_1', thread_key: 't1', kind: 'message', subject: 'Why is order 123 late?', body: 'Do you know?', from: 'Priya', from_assistant: true, link: null, read: false, at: new Date().toISOString(), can_reply: true },
    { id: 'imsg_2', thread_key: 't2', kind: 'notice', subject: 'Support replied', body: 'Bank replied.', from: 'Meera', from_assistant: true, link: null, read: true, at: new Date().toISOString(), can_reply: false },
  ],
};
const sent = {
  items: [
    { id: 'out_1', channel: 'email', to: 'Care (care@bank.example)', subject: 'Failed charge', body: 'Please look.', status: 'needs_you', reason: 'it goes outside your workspace', at: new Date().toISOString(), can_stop: false, can_approve: true },
    { id: 'imsg_9', channel: 'in_app', to: 'Ravi', subject: 'Lunch', body: 'One?', status: 'held', at: new Date().toISOString(), can_stop: true },
  ],
};
const conversations = {
  items: [
    { id: 'wch_1', kind: 'ticket', title: 'Failed charge', with: 'Bigbank', channel: 'email', status: 'waiting', status_label: 'Waiting for a reply', answer: null, updated_at: new Date().toISOString(), ticket: { id: 'tkt_1', company: 'Bigbank', address: 'support@bigbank.example', status: 'waiting', last_summary: '' } },
  ],
};

function mount() {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => {
    const u = String(url);
    if (u.endsWith('/in-app')) return Promise.resolve(json(inbox));
    if (u.endsWith('/in-app/sent')) return Promise.resolve(json(sent));
    if (u.endsWith('/conversations')) return Promise.resolve(json(conversations));
    return Promise.resolve(json({ ok: true, method: init?.method }));
  });
  vi.stubGlobal('fetch', fetchMock);
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>
      <ToastProvider>
        <MemoryRouter initialEntries={['/w/w1/messages']}>
          <Routes>
            <Route path="/w/:ws/messages" element={<MessagesPage />} />
          </Routes>
        </MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>,
  );
  return fetchMock;
}

afterEach(() => vi.unstubAllGlobals());

describe('Messages page', () => {
  it('lists messages and notices, and a reply goes to the right message', async () => {
    const fetchMock = mount();
    const user = userEvent.setup();
    expect(await screen.findByRole('group', { name: 'Why is order 123 late?' })).toBeInTheDocument();
    expect(screen.getByRole('group', { name: 'Support replied' })).toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: 'Reply' })).toHaveLength(1);
    await user.click(screen.getByRole('button', { name: 'Reply' }));
    await user.type(screen.getByRole('textbox', { name: /Your reply to Why is order 123 late/ }), 'The courier lost it.');
    await user.click(screen.getByRole('button', { name: 'Send reply' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/in-app/imsg_1/reply'))).toBe(true));
  });

  it('shows what Meera sent in plain words, with Send it and Stop', async () => {
    const fetchMock = mount();
    const user = userEvent.setup();
    await user.click(await screen.findByRole('tab', { name: /Sent by Meera/ }));
    expect(await screen.findByText('Needs your OK')).toBeInTheDocument();
    expect(screen.getByText(/Waiting for you because it goes outside your workspace/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/in-app/sent/imsg_9/stop'))).toBe(true));
    await user.click(screen.getByRole('button', { name: 'Send it' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/in-app/sent/out_1/approve'))).toBe(true));
  });

  it('lists the conversations Meera is handling and lets the person mark a case resolved', async () => {
    const fetchMock = mount();
    const user = userEvent.setup();
    await user.click(await screen.findByRole('tab', { name: /Conversations Meera is handling/ }));
    expect(await screen.findByText('Waiting for a reply')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Mark resolved' }));
    await waitFor(() => expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith('/support-tickets/tkt_1/status'))).toBe(true));
  });
});
