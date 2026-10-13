import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { EmailBody } from './EmailBody';

describe('EmailBody', () => {
  it('renders HTML in a sandboxed iframe with a CSP and no scripts', () => {
    render(<EmailBody text="plain" html="<p>Hello</p>" />);
    const frame = screen.getByTitle('Email content');
    const sandbox = frame.getAttribute('sandbox') ?? '';
    expect(sandbox).not.toContain('allow-scripts');
    expect(sandbox).not.toContain('allow-forms');
    expect(frame).toHaveAttribute('referrerpolicy', 'no-referrer');
    const doc = frame.getAttribute('srcdoc') ?? '';
    expect(doc).toContain('http-equiv="Content-Security-Policy"');
    expect(doc).toContain('<p>Hello</p>');
    expect(screen.getByText("Images load from the sender's servers.")).toBeInTheDocument();
  });

  it('shows HTML and text-only emails inside the same reader container', () => {
    const { unmount } = render(<EmailBody text="plain" html="<p>Hello</p>" />);
    const htmlRegion = screen.getByRole('region', { name: 'Email body' });
    expect(htmlRegion.className).toBeTruthy();
    expect(within(htmlRegion).getByTitle('Email content')).toBeInTheDocument();
    const htmlClass = htmlRegion.className;
    unmount();

    render(<EmailBody text="only text" html={null} />);
    const textRegion = screen.getByRole('region', { name: 'Email body' });
    expect(textRegion.className).toBe(htmlClass);
    expect(within(textRegion).getByText('only text')).toBeInTheDocument();
    expect(screen.queryByTitle('Email content')).toBeNull();
  });

  it('has no Formatted/Plain toggle; plain text is a tiny overflow option', async () => {
    const user = userEvent.setup();
    render(<EmailBody text="just plain" html="<p>Hello</p>" />);
    expect(screen.queryByRole('button', { name: 'Formatted' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Plain text' })).toBeNull();
    await user.click(screen.getByRole('button', { name: 'More message actions' }));
    await user.click(screen.getByRole('menuitem', { name: 'View plain text' }));
    expect(screen.queryByTitle('Email content')).toBeNull();
    expect(screen.getByText('just plain')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'More message actions' }));
    await user.click(screen.getByRole('menuitem', { name: 'View formatted' }));
    expect(screen.getByTitle('Email content')).toBeInTheDocument();
  });

  it('auto-sizes the frame to its content height on load', () => {
    render(<EmailBody text="t" html="<p>x</p>" />);
    const frame = screen.getByTitle('Email content');
    Object.defineProperty(frame, 'contentDocument', {
      value: { body: { offsetHeight: 742 }, documentElement: { offsetHeight: 742 } },
    });
    fireEvent.load(frame);
    expect(frame).toHaveStyle({ height: '742px' });
  });

  it('shows no overflow menu for text-only emails', () => {
    render(<EmailBody text="only text" html={null} />);
    expect(screen.queryByRole('button', { name: 'More message actions' })).toBeNull();
  });

  it('strips leftover HTML comments and unwraps <https://…> links in text', () => {
    const body = '<!--[if !mso]><!-->Hello there<![endif]-->\n\nOpen <https://example.com/verify?a=1> now\n<!--<![endif]-->';
    const { container } = render(<EmailBody text={body} html={null} />);
    expect(container.textContent).not.toContain('<!--');
    expect(container.textContent).not.toContain('endif');
    expect(container.textContent).not.toContain('<https');
    const link = screen.getByRole('link', { name: 'https://example.com/verify?a=1' });
    expect(link).toHaveAttribute('href', 'https://example.com/verify?a=1');
    expect(screen.getByText(/Hello there/)).toBeInTheDocument();
  });

  it('collapses quoted history behind a button and reveals it on click', async () => {
    const user = userEvent.setup();
    const body = 'Sounds good, thanks!\n\nOn Mon, Oct 13, 2025 at 10:25 PM Bob <bob@example.com> wrote:\n> Can you confirm Friday?\n> Thanks';
    render(<EmailBody text={body} html={null} />);
    expect(screen.getByText('Sounds good, thanks!')).toBeInTheDocument();
    expect(screen.queryByText(/Can you confirm Friday/)).toBeNull();
    const toggle = screen.getByRole('button', { name: 'Show trimmed content' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await user.click(toggle);
    expect(screen.getByText(/Can you confirm Friday/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Hide trimmed content' })).toHaveAttribute('aria-expanded', 'true');
  });

  it('dims the signature', () => {
    render(<EmailBody text={'Thanks,\nAda\n-- \nAda Lovelace\nEngineer'} html={null} />);
    expect(screen.getByText(/Ada Lovelace/)).toHaveClass('text-ink-muted');
  });
});
