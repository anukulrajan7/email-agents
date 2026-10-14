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
});
