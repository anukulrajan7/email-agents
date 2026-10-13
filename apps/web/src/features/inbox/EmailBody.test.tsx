import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { EmailBody } from './EmailBody';

describe('EmailBody', () => {
  it('renders HTML in a locked-down sandboxed iframe with a CSP', () => {
    render(<EmailBody text="plain" html="<p>Hello</p>" />);
    const frame = screen.getByTitle('Email content');
    const sandbox = frame.getAttribute('sandbox') ?? '';
    expect(sandbox).not.toContain('allow-scripts');
    expect(sandbox).not.toContain('allow-same-origin');
    expect(sandbox).not.toContain('allow-forms');
    expect(frame).toHaveAttribute('referrerpolicy', 'no-referrer');
    const doc = frame.getAttribute('srcdoc') ?? '';
    expect(doc).toContain('http-equiv="Content-Security-Policy"');
    expect(doc).toContain('<p>Hello</p>');
    expect(screen.getByText("Images load from the sender's servers.")).toBeInTheDocument();
  });

  it('switches to plain text and back', async () => {
    const user = userEvent.setup();
    render(<EmailBody text="just plain" html="<p>Hello</p>" />);
    await user.click(screen.getByRole('button', { name: 'Plain text' }));
    expect(screen.queryByTitle('Email content')).toBeNull();
    expect(screen.getByText('just plain')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Formatted' }));
    expect(screen.getByTitle('Email content')).toBeInTheDocument();
  });

  it('expands and collapses the frame', async () => {
    const user = userEvent.setup();
    render(<EmailBody text="t" html="<p>x</p>" />);
    const frame = screen.getByTitle('Email content');
    expect(frame).toHaveStyle({ height: '600px' });
    await user.click(screen.getByRole('button', { name: 'Expand' }));
    expect(frame).toHaveStyle({ height: '1600px' });
    expect(screen.getByRole('button', { name: 'Collapse' })).toBeInTheDocument();
  });

  it('falls back to plain text without HTML', () => {
    render(<EmailBody text="only text" html={null} />);
    expect(screen.queryByTitle('Email content')).toBeNull();
    expect(screen.getByText('only text')).toBeInTheDocument();
  });
});
