import { describe, expect, it } from 'vitest';
import { purgeSentence } from './IntegrationsPage';

describe('purgeSentence', () => {
  it('states the exact counts and what stays', () => {
    expect(purgeSentence({ emails: 1368, attachments: 120, drafts: 45, summaries: 1200 })).toBe(
      "1,368 emails, 120 attachments, 45 drafts and their summaries will be permanently deleted from Post Room. Labels, settings and other mailboxes stay. This can't be undone.",
    );
  });

  it('uses the singular for one', () => {
    expect(purgeSentence({ emails: 1, attachments: 1, drafts: 1, summaries: 0 })).toMatch(/^1 email, 1 attachment, 1 draft and/);
  });
});
