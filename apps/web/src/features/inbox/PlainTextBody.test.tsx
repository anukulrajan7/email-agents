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
