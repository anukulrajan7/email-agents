import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TOKEN_KEY } from '../../api/client';
import { ToastProvider } from '../../components/ui';
import { AppShell } from '../../app/AppShell';
import { AskMeera } from './AskMeera';
import type { CommandResponse } from './types';

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

const answer = (over: Partial<CommandResponse> = {}): CommandResponse => ({
  answer: 'Priya is asking when her offer letter will be ready.',
  citations: [{ message_id: 'm1', subject: 'Offer letter for Priya' }],
  plan: [],
  over_budget: false,
  ...over,
});

const draftCard = {
  id: 'apl_1',
  action: 'draft_reply' as const,
  summary: 'Draft a reply to “Offer letter for Priya”',
  params: { message_id: 'm1' },
};

type Handler = (url: string, init?: RequestInit) => Response | undefined;

function mockFetch(handler: Handler) {
  const fetchMock = vi.fn((url: string, init?: RequestInit) => Promise.resolve(handler(url, init) ?? json({ items: [] })));
  vi.stubGlobal('fetch', fetchMock);
  return fetchMock;
}

const callsTo = (fetchMock: ReturnType<typeof mockFetch>, suffix: string) =>
  fetchMock.mock.calls.filter(([url]) => String(url).endsWith(suffix)) as [string, RequestInit][];

function renderAssistant() {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}>
      <ToastProvider>
        <MemoryRouter initialEntries={['/w/w1/inbox']}>
          <Routes>
            <Route path="/w/:ws/*" element={<AskMeera ws="w1" />} />
          </Routes>
        </MemoryRouter>
      </ToastProvider>
    </QueryClientProvider>,
  );
}

async function openPanel() {
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Ask Meera' }));
  const panel = await screen.findByRole('dialog', { name: 'Ask Meera' });
  return { user, panel };
}

class FakeRecorder {
  static isTypeSupported = () => true;
  state: 'inactive' | 'recording' = 'inactive';
  mimeType = 'audio/webm';
  ondataavailable: ((e: { data: Blob }) => void) | null = null;
  onstop: (() => void) | null = null;
  constructor(readonly stream: MediaStream) {}
  start() {
    this.state = 'recording';
  }
  stop() {
    this.state = 'inactive';
    this.ondataavailable?.({ data: new Blob(['voice'], { type: 'audio/webm' }) });
    this.onstop?.();
  }
}

function stubMic(getUserMedia: () => Promise<unknown>) {
  vi.stubGlobal('MediaRecorder', FakeRecorder);
  Object.defineProperty(navigator, 'mediaDevices', { value: { getUserMedia }, configurable: true });
}

describe('Ask Meera', () => {
  beforeEach(() => localStorage.setItem(TOKEN_KEY, 'tok'));
  afterEach(() => {
    vi.unstubAllGlobals();
    Object.defineProperty(navigator, 'mediaDevices', { value: undefined, configurable: true });
    localStorage.clear();
  });

  it('answers a typed command and links its sources', async () => {
    const fetchMock = mockFetch((url) => (url.endsWith('/assistant/command') ? json(answer()) : undefined));
    renderAssistant();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    const { user, panel } = await openPanel();

    await user.type(within(panel).getByRole('textbox', { name: 'Message to Meera' }), 'What is Priya waiting for?');
    await user.click(within(panel).getByRole('button', { name: 'Send' }));

    expect(await within(panel).findByText(/offer letter will be ready/)).toBeInTheDocument();
    expect(within(panel).getByText('What is Priya waiting for?')).toBeInTheDocument();
    expect(within(panel).getByRole('link', { name: 'Offer letter for Priya' })).toHaveAttribute('href', '/w/w1/inbox/m1');
    const [url, init] = callsTo(fetchMock, '/assistant/command')[0]!;
    expect(url).toBe('/api/v1/workspaces/w1/assistant/command');
    expect(JSON.parse(String(init.body))).toEqual({ text: 'What is Priya waiting for?' });
  });

  it('shows a plan card that only runs after Confirm, then toasts the result', async () => {
    const fetchMock = mockFetch((url) => {
      if (url.endsWith('/assistant/command')) return json(answer({ answer: 'I prepared a draft for you to confirm.', citations: [], plan: [draftCard] }));
      if (url.endsWith('/assistant/confirm')) {
        return json({ ok: true, action: 'draft_reply', message: 'Drafting the reply. It will appear on the email shortly.', link: { kind: 'message', id: 'm1' } });
      }
      return undefined;
    });
    renderAssistant();
    const { user, panel } = await openPanel();

    await user.type(within(panel).getByRole('textbox', { name: 'Message to Meera' }), 'draft a reply to Priya{Enter}');
    const card = await within(panel).findByRole('article', { name: /Draft a reply: Draft a reply to “Offer letter for Priya”/ });
    expect(callsTo(fetchMock, '/assistant/confirm')).toHaveLength(0);

    await user.click(within(card).getByRole('button', { name: /^Confirm/ }));
    await waitFor(() => expect(within(card).getByText(/Drafting the reply/)).toBeInTheDocument());
    const [url, init] = callsTo(fetchMock, '/assistant/confirm')[0]!;
    expect(url).toBe('/api/v1/workspaces/w1/assistant/confirm');
    expect(JSON.parse(String(init.body))).toEqual({ plan_item_id: 'apl_1' });
    expect(screen.getAllByText(/Drafting the reply/)).toHaveLength(2);
    expect(within(card).getByRole('link', { name: 'Open the email' })).toHaveAttribute('href', '/w/w1/inbox/m1');
    expect(within(card).queryByRole('button', { name: /^Confirm/ })).not.toBeInTheDocument();
  });

  it('cancels a plan card without calling the server, and shows a failed confirm', async () => {
    const fetchMock = mockFetch((url) => {
      if (url.endsWith('/assistant/command')) return json(answer({ citations: [], plan: [draftCard, { ...draftCard, id: 'apl_2', action: 'snooze_alert', summary: 'Snooze the alert “Late reply” for 4 hours' }] }));
      if (url.endsWith('/assistant/confirm')) return json({ error: { code: 'conflict', message: 'This action expired; ask again', request_id: 'r' } }, 409);
      return undefined;
    });
    renderAssistant();
    const { user, panel } = await openPanel();
    await user.type(within(panel).getByRole('textbox', { name: 'Message to Meera' }), 'do both{Enter}');

    const first = await within(panel).findByRole('article', { name: /Draft a reply/ });
    await user.click(within(first).getByRole('button', { name: /^Cancel/ }));
    expect(within(first).getByText(/Nothing was done/)).toBeInTheDocument();
    expect(callsTo(fetchMock, '/assistant/confirm')).toHaveLength(0);

    const second = within(panel).getByRole('article', { name: /Snooze alert/ });
    await user.click(within(second).getByRole('button', { name: /^Confirm/ }));
    expect(await within(second).findByRole('alert')).toHaveTextContent('This action expired; ask again');
    expect(within(second).getByRole('button', { name: /^Confirm/ })).toBeEnabled();
  });

  it('opens with Ctrl+K, focuses the input and closes with Escape', async () => {
    mockFetch(() => undefined);
    renderAssistant();
    const user = userEvent.setup();
    await user.keyboard('{Control>}k{/Control}');
    const panel = await screen.findByRole('dialog', { name: 'Ask Meera' });
    await waitFor(() => expect(within(panel).getByRole('textbox', { name: 'Message to Meera' })).toHaveFocus());
    await user.keyboard('{Escape}');
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Ask Meera' })).not.toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Ask Meera' })).toHaveFocus();
  });

  it('records on the mic button, uploads the clip and sends the server transcript as the command', async () => {
    const track = { stop: vi.fn() };
    const getUserMedia = vi.fn(() => Promise.resolve({ getTracks: () => [track] }));
    stubMic(getUserMedia);
    const fetchMock = mockFetch((url) => {
      if (url.endsWith('/assistant/transcribe')) return json({ text: 'what needs a reply' });
      if (url.endsWith('/assistant/command')) return json(answer({ answer: 'Two emails need a reply.', citations: [] }));
      return undefined;
    });
    renderAssistant();
    const { user, panel } = await openPanel();

    const mic = within(panel).getByRole('button', { name: 'Talk to Meera' });
    expect(mic).toHaveAttribute('aria-pressed', 'false');
    await user.click(mic);
    await waitFor(() => expect(within(panel).getByRole('button', { name: 'Talk to Meera' })).toHaveAttribute('aria-pressed', 'true'));
    expect(within(panel).getByText(/Listening/)).toBeInTheDocument();

    await user.click(within(panel).getByRole('button', { name: 'Talk to Meera' }));
    expect(await within(panel).findByText('Two emails need a reply.')).toBeInTheDocument();
    expect(within(panel).getByRole('button', { name: 'Talk to Meera' })).toHaveAttribute('aria-pressed', 'false');
    expect(track.stop).toHaveBeenCalled();

    const [url, init] = callsTo(fetchMock, '/assistant/transcribe')[0]!;
    expect(url).toBe('/api/v1/workspaces/w1/assistant/transcribe');
    expect(init.body).toBeInstanceOf(FormData);
    const form = init.body as FormData;
    expect((form.get('audio') as Blob).size).toBeGreaterThan(0);
    expect(Number(form.get('duration_ms'))).toBeGreaterThanOrEqual(0);
    expect((init.headers as Record<string, string>)['Content-Type']).toBeUndefined();
    expect(JSON.parse(String(callsTo(fetchMock, '/assistant/command')[0]![1].body))).toEqual({ text: 'what needs a reply' });
    expect(within(panel).getByText('what needs a reply')).toBeInTheDocument();
  });

  it('keeps working as text only when the microphone is blocked', async () => {
    stubMic(() => Promise.reject(new DOMException('denied', 'NotAllowedError')));
    mockFetch(() => undefined);
    renderAssistant();
    const { user, panel } = await openPanel();

    await user.click(within(panel).getByRole('button', { name: 'Talk to Meera' }));
    expect(await screen.findByText('The microphone is blocked. Type your request instead.')).toBeInTheDocument();
    expect(within(panel).getByRole('button', { name: 'Talk to Meera' })).toHaveAttribute('aria-pressed', 'false');
    expect(within(panel).getByRole('textbox', { name: 'Message to Meera' })).toBeEnabled();
  });

  it('disables the mic where recording is not supported', async () => {
    mockFetch(() => undefined);
    renderAssistant();
    const { panel } = await openPanel();
    expect(within(panel).getByRole('button', { name: 'Voice input is not available in this browser' })).toBeDisabled();
    expect(within(panel).getByRole('textbox', { name: 'Message to Meera' })).toBeEnabled();
  });

  it('reads answers aloud only when asked to', async () => {
    const speak = vi.fn();
    vi.stubGlobal('SpeechSynthesisUtterance', class { constructor(readonly text: string) {} });
    Object.defineProperty(window, 'speechSynthesis', { value: { speak, cancel: vi.fn() }, configurable: true });
    mockFetch((url) => (url.endsWith('/assistant/command') ? json(answer({ citations: [] })) : undefined));
    renderAssistant();
    const { user, panel } = await openPanel();
    const box = within(panel).getByRole('textbox', { name: 'Message to Meera' });

    await user.type(box, 'first{Enter}');
    await within(panel).findByText(/offer letter will be ready/);
    expect(speak).not.toHaveBeenCalled();

    await user.click(within(panel).getByRole('switch', { name: 'Read answers aloud' }));
    await user.type(box, 'second{Enter}');
    await waitFor(() => expect(speak).toHaveBeenCalledTimes(1));
    expect((speak.mock.calls[0]![0] as { text: string }).text).toMatch(/offer letter will be ready/);
    Reflect.deleteProperty(window, 'speechSynthesis');
  });

  it('is available on every page inside the app shell', async () => {
    mockFetch((url) =>
      url.endsWith('/me')
        ? json({ user: { id: 'u1', email: 'priya@acme.test', name: 'Priya' }, workspaces: [{ id: 'w1', name: 'Acme', role: 'operator', created_at: '' }], auth_mode: 'password' })
        : json({ items: [], pending_count: 0, open_count: 0 }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <MemoryRouter initialEntries={['/w/w1/inbox']}>
          <Routes>
            <Route path="/w/:ws" element={<AppShell />}>
              <Route path="inbox" element={<p>Inbox page</p>} />
            </Route>
          </Routes>
        </MemoryRouter>
      </QueryClientProvider>,
    );
    expect(await screen.findByRole('button', { name: 'Ask Meera' })).toBeInTheDocument();
  });
});
