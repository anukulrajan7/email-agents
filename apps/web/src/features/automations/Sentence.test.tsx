import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { AutomationSentence } from './Sentence';
import { validateAutomation } from './validate';
import type { AutomationInput } from './types';

const names: Record<string, string> = { timesheet: 'Timesheet' };
const labelName = (key: string) => names[key] ?? key;

describe('AutomationSentence', () => {
  it('reads as a sentence with bold conditions and actions', () => {
    const { container } = render(
      <AutomationSentence
        conditions={[
          { field: 'needs_reply', op: 'is', value: true },
          { field: 'label', op: 'is', value: 'timesheet' },
        ]}
        actions={[{ type: 'draft_reply' }, { type: 'save_to_gmail_drafts' }]}
        labelName={labelName}
      />,
    );
    expect(container).toHaveTextContent('When an email needs a reply and label is Timesheet → then Draft a reply, Save to Gmail drafts');
    expect([...container.querySelectorAll('strong')].map((s) => s.textContent)).toEqual([
      'needs a reply',
      'is Timesheet',
      'Draft a reply',
      'Save to Gmail drafts',
    ]);
  });

  it('handles no conditions, negation and text conditions', () => {
    const { container } = render(
      <AutomationSentence
        conditions={[
          { field: 'label', op: 'is_not', value: 'timesheet' },
          { field: 'from', op: 'contains', value: 'acme' },
        ]}
        actions={[{ type: 'remind_if_no_reply', hours: 4 }]}
        labelName={labelName}
      />,
    );
    expect(container).toHaveTextContent('label is not Timesheet and sender contains “acme”');
    expect(container).toHaveTextContent('Remind me if no reply in 4 hours');
  });
});

const base: AutomationInput = { name: 'Test', enabled: true, conditions: [], actions: [] };

describe('validateAutomation', () => {
  it('requires a draft before saving to Gmail drafts', () => {
    const issues = validateAutomation({ ...base, actions: [{ type: 'save_to_gmail_drafts' }, { type: 'draft_reply' }] });
    expect(issues.map((i) => i.id)).toEqual(['action-0']);
  });

  it('requires a draft, a cap and valid domains before send_reply', () => {
    const issues = validateAutomation({
      ...base,
      actions: [{ type: 'send_reply', daily_cap: 0, allowed_domains: [] }],
    });
    expect(issues.map((i) => i.id)).toEqual(['action-0', 'action-0-cap', 'action-0-domains']);
  });

  it('accepts a well-formed send_reply', () => {
    expect(
      validateAutomation({
        ...base,
        actions: [{ type: 'draft_reply' }, { type: 'send_reply', daily_cap: 5, allowed_domains: ['acme.com'] }],
      }),
    ).toEqual([]);
  });

  it('flags blank names, empty actions and empty condition values', () => {
    const issues = validateAutomation({ ...base, name: ' ', conditions: [{ field: 'subject', op: 'contains', value: '' }] });
    expect(issues.map((i) => i.id)).toEqual(['name', 'actions', 'condition-0']);
  });
});
