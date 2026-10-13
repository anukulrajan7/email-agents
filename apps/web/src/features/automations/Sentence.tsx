import { Fragment } from 'react';
import type { Action, Condition } from './types';

export function conditionParts(c: Condition, labelName: (key: string) => string): { plain: string; strong: string } {
  switch (c.field) {
    case 'label':
      return { plain: 'label', strong: `${c.op === 'is' ? 'is' : 'is not'} ${labelName(c.value)}` };
    case 'priority':
      return { plain: 'priority', strong: `is ${c.value}` };
    case 'needs_reply':
      return { plain: '', strong: c.value ? 'needs a reply' : 'does not need a reply' };
    case 'from':
      return { plain: 'sender', strong: `contains “${c.value}”` };
    case 'subject':
      return { plain: 'subject', strong: `contains “${c.value}”` };
  }
}

export function actionPhrase(a: Action): string {
  switch (a.type) {
    case 'draft_reply':
      return 'Draft a reply';
    case 'save_to_gmail_drafts':
      return 'Save to Gmail drafts';
    case 'send_reply':
      return `Send the reply (up to ${a.daily_cap} a day, only to ${a.allowed_domains.join(', ') || 'no domains yet'})`;
    case 'create_review':
      return 'Create a review task';
    case 'remind_if_no_reply':
      return `Remind me if no reply in ${a.hours} ${a.hours === 1 ? 'hour' : 'hours'}`;
  }
}

export function AutomationSentence({
  conditions,
  actions,
  labelName,
  className,
}: {
  conditions: Condition[];
  actions: Action[];
  labelName: (key: string) => string;
  className?: string;
}) {
  return (
    <p className={className ?? 'text-[15px] leading-7 text-ink'}>
      {conditions.length === 0 ? 'When any email arrives' : 'When an email '}
      {conditions.map((c, i) => {
        const { plain, strong } = conditionParts(c, labelName);
        return (
          <Fragment key={i}>
            {i > 0 && ' and '}
            {plain && `${plain} `}
            <strong className="font-semibold">{strong}</strong>
          </Fragment>
        );
      })}{' '}
      <span aria-hidden="true">→</span>
      <span className="sr-only"> then </span>{' '}
      {actions.length === 0 && <span className="text-ink-muted">do nothing yet</span>}
      {actions.map((a, i) => (
        <Fragment key={i}>
          {i > 0 && ', '}
          <strong className="font-semibold">{actionPhrase(a)}</strong>
        </Fragment>
      ))}
    </p>
  );
}
