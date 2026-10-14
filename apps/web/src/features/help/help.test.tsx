import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import { HELP, type HelpTopic } from './content';
import { HelpPopover } from './HelpPopover';

afterEach(() => localStorage.clear());

describe('HelpPopover', () => {
  it('opens on click, names its topic, and closes on Escape returning focus', async () => {
    const user = userEvent.setup();
    render(<HelpPopover topic="autonomy" />);
    const button = screen.getByRole('button', { name: 'What is this?' });
    expect(button).toHaveAttribute('aria-expanded', 'false');
    await user.click(button);
    expect(button).toHaveAttribute('aria-expanded', 'true');
    const region = screen.getByRole('region', { name: HELP.autonomy.title });
    expect(region).toHaveTextContent('10-minute hold');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('region')).not.toBeInTheDocument();
    expect(button).toHaveFocus();
  });

  it('closes when you click elsewhere', async () => {
    const user = userEvent.setup();
    render(
      <>
        <HelpPopover topic="packs" />
        <p>elsewhere</p>
      </>,
    );
    await user.click(screen.getByRole('button', { name: 'What is this?' }));
    await user.click(screen.getByText('elsewhere'));
    expect(screen.queryByRole('region')).not.toBeInTheDocument();
  });

  it('has plain-English help for every page topic, with no model names', () => {
    const topics = Object.keys(HELP) as HelpTopic[];
    expect(topics).toEqual(expect.arrayContaining(['agents', 'autonomy', 'packs', 'memory', 'approvals', 'hrms']));
    expect(JSON.stringify(HELP)).not.toMatch(/gpt|claude|gemini|anthropic|openai|haiku|sonnet|opus/i);
  });
});
