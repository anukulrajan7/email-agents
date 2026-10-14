import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AgentFace } from './AgentFace';

function stubMotion(reduce: boolean) {
  vi.stubGlobal(
    'matchMedia',
    vi.fn(() => ({ matches: reduce, addEventListener: vi.fn(), removeEventListener: vi.fn() })),
  );
  Object.defineProperty(window, 'matchMedia', { configurable: true, value: globalThis.matchMedia });
}

describe('AgentFace', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('idles gently by default', () => {
    stubMotion(false);
    render(<AgentFace avatar="owl" />);
    expect(screen.getByTestId('agent-face')).toHaveAttribute('data-motion', 'idle');
    expect(screen.getByTestId('agent-face').querySelector('img')).toHaveAttribute('src', '/avatars/owl.webp');
  });

  it('stays still when the user prefers reduced motion', () => {
    stubMotion(true);
    render(<AgentFace avatar="owl" />);
    expect(screen.getByTestId('agent-face')).toHaveAttribute('data-motion', 'still');
  });

  it('is decorative for screen readers', () => {
    stubMotion(false);
    render(<AgentFace avatar="owl" />);
    expect(screen.getByTestId('agent-face')).toHaveAttribute('aria-hidden', 'true');
  });
});
