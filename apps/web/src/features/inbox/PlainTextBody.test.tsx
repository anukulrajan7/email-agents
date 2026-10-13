import { render, screen } from '@testing-library/react';
import { PlainTextBody } from './PlainTextBody';

describe('PlainTextBody', () => {
  it('renders HTML-looking content as literal text without creating elements', () => {
    const body = '<img src=x onerror="alert(1)"><script>alert(2)</script>Hello';
    const { container } = render(<PlainTextBody text={body} />);
    const pre = container.querySelector('pre');
    expect(pre).toHaveClass('whitespace-pre-wrap');
    expect(pre?.textContent).toBe(body);
    expect(container.querySelector('img, script')).toBeNull();
    expect(screen.getByText(/onerror/)).toBeInTheDocument();
  });
});

describe('PlainTextBody links and quotes', () => {
  it('linkifies URLs and emails without injecting HTML', () => {
    const body = 'See https://example.com/a?b=1, or mail bob@example.com. <b>x</b>';
    const { container } = render(<PlainTextBody text={body} />);
    const links = container.querySelectorAll('a');
    expect(links).toHaveLength(2);
    expect(links[0]).toHaveAttribute('href', 'https://example.com/a?b=1');
    expect(links[0]).toHaveAttribute('target', '_blank');
    expect(links[0]).toHaveAttribute('rel', 'noopener noreferrer nofollow');
    expect(links[1]).toHaveAttribute('href', 'mailto:bob@example.com');
    expect(container.querySelector('b')).toBeNull();
    expect(container.querySelector('pre')?.textContent).toBe(body);
  });

  it('collapses quoted lines into a details block', () => {
    const { container } = render(<PlainTextBody text={'Thanks!\n> old line 1\n> old line 2\nBye'} />);
    const details = container.querySelector('details');
    expect(details).not.toBeNull();
    expect(details).not.toHaveAttribute('open');
    expect(screen.getByText('Show quoted text')).toBeInTheDocument();
    expect(details?.textContent).toContain('> old line 2');
  });
});
