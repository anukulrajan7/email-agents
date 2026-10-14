import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ToastProvider } from '../../components/ui';
import { agent, json } from '../../test/fixtures';
import { TriggersTab } from './TriggersTab';

const rohan = agent({ id: 'a-rohan', name: 'Rohan', category: 'hrms', template_key: 'hrms.timesheets' });
const spec = {
  ...rohan,
  spec: {
    triggers: [{ type: 'email_received', match: { labels: ['timesheet'], from_domains: [], from_addresses: [], subject_keywords: [], needs_reply: null, priority: null, attachment_types: [] } }],
  },
  versions: [{ version: 3, active: true }],
};

const nth = (els: HTMLElement[], i: number) => els[i] as HTMLElement;
const m = (over: object) => ({ labels: [], from_domains: [], from_addresses: [], subject_keywords: [], needs_reply: null, priority: null, attachment_types: [], ...over });
const ishaan = agent({ id: 'a-ishaan', name: 'Ishaan', category: 'support', template_key: 'support.resolver' });
const twoRules = {
  ...ishaan,
  spec: {
    triggers: [
      { type: 'email_received', match: m({ labels: ['question', 'complaint'] }) },
      { type: 'schedule', every_hours: 24 },
      { type: 'email_received', match: m({ subject_keywords: ['escalate', 'unacceptable', 'legal action'] }) },
    ],
  },
  versions: [{ version: 4, active: true }],
};

function mount(a: typeof rohan, body: unknown, puts: string[]) {
  vi.stubGlobal(
    'fetch',
    vi.fn((_url: string, init?: RequestInit) => {
      if (init?.method === 'PUT') puts.push(String(init.body));
      return Promise.resolve(json(body));
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <ToastProvider>
        <TriggersTab ws="w1" agent={a} canEdit />
      </ToastProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => vi.unstubAllGlobals());

describe('TriggersTab', () => {
  it('edits the trigger lists and saves a new version of the spec', async () => {
    const puts: string[] = [];
    vi.stubGlobal(
      'fetch',
      vi.fn((url: string, init?: RequestInit) => {
        if (init?.method === 'PUT') {
          puts.push(`${url} ${String(init.body)}`);
          return Promise.resolve(json(spec));
        }
        return Promise.resolve(json(spec));
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <ToastProvider>
          <TriggersTab ws="w1" agent={rohan} canEdit />
        </ToastProvider>
      </QueryClientProvider>,
    );
    const labels = await screen.findByLabelText('Labels');
    expect(labels).toHaveValue('timesheet');
    expect(screen.getByRole('button', { name: 'Save triggers' })).toBeDisabled();
    await userEvent.type(await screen.findByLabelText('Subject words'), 'hours, week');
    await userEvent.click(screen.getByRole('button', { name: 'Save triggers' }));
    await waitFor(() => expect(puts).toHaveLength(1));
    expect(puts[0]).toContain('/agents/a-rohan/spec');
    expect(puts[0]).toContain('"subject_keywords":["hours","week"]');
    expect(puts[0]).toContain('"labels":["timesheet"]');
  });

  it('shows one editor per email trigger and keeps all of them, in order, with other triggers untouched', async () => {
    const puts: string[] = [];
    mount(ishaan, twoRules, puts);
    expect(await screen.findByText('Rule 1')).toBeInTheDocument();
    expect(screen.getByText('Rule 2')).toBeInTheDocument();
    const subjects = screen.getAllByLabelText('Subject words');
    expect(subjects[1]).toHaveValue('escalate, unacceptable, legal action');
    await userEvent.type(nth(screen.getAllByLabelText('Labels'), 0), ', refund_request');
    await userEvent.click(screen.getByRole('button', { name: 'Save triggers' }));
    await waitFor(() => expect(puts).toHaveLength(1));
    const saved = JSON.parse(puts[0] ?? '{}').spec.triggers;
    expect(saved.map((t: { type: string }) => t.type)).toEqual(['email_received', 'schedule', 'email_received']);
    expect(saved[0].match.labels).toEqual(['question', 'complaint', 'refund_request']);
    expect(saved[1]).toEqual({ type: 'schedule', every_hours: 24 });
    expect(saved[2].match.subject_keywords).toEqual(['escalate', 'unacceptable', 'legal action']);
  });

  it('adds and removes rules and warns that a save makes the helper ask first again', async () => {
    const puts: string[] = [];
    mount(ishaan, twoRules, puts);
    await screen.findByText('Rule 2');
    expect(screen.getByText(/ask first again until it earns its level back/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Remove rule 2' }));
    await userEvent.click(screen.getByRole('button', { name: 'Add rule' }));
    await userEvent.type(nth(screen.getAllByLabelText('Sender domains'), 1), 'acme.com');
    await userEvent.click(screen.getByRole('button', { name: 'Save triggers' }));
    await waitFor(() => expect(puts).toHaveLength(1));
    const saved = JSON.parse(puts[0] ?? '{}').spec.triggers;
    expect(saved.map((t: { type: string }) => t.type)).toEqual(['email_received', 'schedule', 'email_received']);
    expect(saved[2].match.from_domains).toEqual(['acme.com']);
    expect(saved[2].match.subject_keywords).toEqual([]);
  });

  it('will not save a rule with every box empty', async () => {
    const puts: string[] = [];
    mount(ishaan, twoRules, puts);
    await screen.findByText('Rule 2');
    await userEvent.click(screen.getByRole('button', { name: 'Add rule' }));
    await userEvent.type(nth(screen.getAllByLabelText('Labels'), 2), 'x');
    await userEvent.clear(nth(screen.getAllByLabelText('Labels'), 2));
    expect(screen.getByRole('alert')).toHaveTextContent('Fill in at least one box in every rule');
    expect(screen.getByRole('button', { name: 'Save triggers' })).toBeDisabled();
    expect(puts).toHaveLength(0);
  });
});
