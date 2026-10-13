import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { Label } from '../../api/types';
import { AutomationBuilder } from './Builder';
import type { Automation, AutomationInput } from './types';

const labels: Label[] = [
  { id: 'l1', key: 'timesheet', name: 'Timesheet', description: 'Weekly timesheets', color: 'slate', position: 1, system: false, archived: false, message_count: 1 },
];

const saved: Automation = {
  id: 'a1',
  name: 'Ack timesheets',
  enabled: true,
  trigger: 'email_received',
  preset_key: null,
  conditions: [{ field: 'label', op: 'is', value: 'timesheet' }],
  actions: [{ type: 'draft_reply' }],
  run_count: 0,
  last_run_at: null,
  created_at: '',
  updated_at: '',
};

function setup(initial?: Automation) {
  const fetchMock = vi.fn((_url: string, _init?: RequestInit) =>
    Promise.resolve(new Response(JSON.stringify({ matched: [{ message_id: 'm1', subject: 'Timesheet week 40' }] }))),
  );
  vi.stubGlobal('fetch', fetchMock);
  const onSave = vi.fn<(input: AutomationInput) => void>();
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <MemoryRouter>
        <AutomationBuilder ws="w1" initial={initial} labels={labels} saving={false} error={undefined} onSave={onSave} onCancel={() => {}} />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return { onSave, fetchMock, user: userEvent.setup() };
}

describe('AutomationBuilder', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('blocks send_reply without a draft_reply before it and shows the auto-send warning', async () => {
    const { onSave, user } = setup();
    await user.type(screen.getByLabelText('Name'), 'Auto reply');
    await user.click(screen.getByRole('button', { name: '+ Send the reply' }));
    expect(screen.getByText(/Sends automatically\. Skipped when the draft has missing info \[NEEDS\] or the daily cap is reached\./)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Create automation' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('needs “Draft a reply” before it');
    expect(onSave).not.toHaveBeenCalled();
  });

  it('requires a valid sender domain for send_reply, then saves in step order', async () => {
    const { onSave, user } = setup();
    await user.type(screen.getByLabelText('Name'), 'Auto reply');
    await user.click(screen.getByRole('button', { name: '+ Send the reply' }));
    await user.click(screen.getByRole('button', { name: '+ Draft a reply' }));
    await user.click(screen.getByRole('button', { name: 'Move step 2 up' }));
    await user.click(screen.getByRole('button', { name: 'Create automation' }));
    expect(onSave).not.toHaveBeenCalled();
    expect(await screen.findByRole('alert')).toHaveTextContent('Step 2');

    const domain = screen.getByLabelText('Only auto-send to these sender domains');
    await user.type(domain, 'me@acme.com');
    await user.click(screen.getByRole('button', { name: 'Add domain' }));
    expect(screen.getAllByRole('alert').some((a) => /without “@”/.test(a.textContent ?? ''))).toBe(true);
    await user.clear(domain);
    await user.type(domain, 'Acme.com{Enter}');
    expect(screen.getByRole('list', { name: 'Allowed domains' })).toHaveTextContent('acme.com');

    await user.click(screen.getByRole('button', { name: 'Create automation' }));
    expect(onSave).toHaveBeenCalledWith({
      name: 'Auto reply',
      enabled: true,
      conditions: [],
      actions: [{ type: 'draft_reply' }, { type: 'send_reply', daily_cap: 20, allowed_domains: ['acme.com'] }],
    });
  });

  it('adds a condition and saves it', async () => {
    const { onSave, user } = setup();
    await user.type(screen.getByLabelText('Name'), 'Chase');
    await user.click(screen.getByRole('button', { name: '+ Add a condition' }));
    await user.selectOptions(screen.getByLabelText('Condition 1 field'), 'priority');
    await user.click(screen.getByRole('button', { name: '+ Create a review task' }));
    await user.click(screen.getByRole('button', { name: 'Create automation' }));
    expect(onSave).toHaveBeenCalledWith({
      name: 'Chase',
      enabled: true,
      conditions: [{ field: 'priority', op: 'is', value: 'high' }],
      actions: [{ type: 'create_review' }],
    });
  });

  it('disables testing for an unsaved automation and lists matches for a saved one', async () => {
    const unsaved = setup();
    expect(screen.getByRole('button', { name: 'Test on recent emails' })).toBeDisabled();
    expect(screen.getByText(/Save this automation first/)).toBeInTheDocument();
    expect(unsaved.fetchMock).not.toHaveBeenCalled();
  });

  it('tests a saved automation against recent emails', async () => {
    const { fetchMock, user } = setup(saved);
    await user.click(screen.getByRole('button', { name: 'Test on recent emails' }));
    expect(await screen.findByText('Timesheet week 40')).toBeInTheDocument();
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('/api/v1/workspaces/w1/automations/a1/test');
    expect(init?.method).toBe('POST');
  });
});
