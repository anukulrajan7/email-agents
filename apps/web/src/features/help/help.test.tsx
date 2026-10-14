import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import { HELP, type HelpTopic } from './content';
import { FirstRunTour } from './FirstRunTour';
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

describe('FirstRunTour', () => {
  it('walks through the steps and is remembered once finished', async () => {
    const user = userEvent.setup();
    const { unmount } = render(<FirstRunTour />);
    expect(screen.getByRole('region', { name: 'Quick tour' })).toHaveTextContent('1 of 4');
    await user.click(screen.getByRole('button', { name: 'Next' }));
    expect(screen.getByText('They ask before sending')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByText('Meet your agents')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Skip tour' }));
    expect(screen.queryByRole('region', { name: 'Quick tour' })).not.toBeInTheDocument();
    unmount();
    render(<FirstRunTour />);
    expect(screen.queryByRole('region', { name: 'Quick tour' })).not.toBeInTheDocument();
  });

  it('can be finished with the last step', async () => {
    const user = userEvent.setup();
    render(<FirstRunTour />);
    for (let i = 0; i < 3; i++) await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(screen.getByRole('button', { name: 'Got it' }));
    expect(localStorage.getItem('tour:v1:done')).toBe('1');
  });
});
