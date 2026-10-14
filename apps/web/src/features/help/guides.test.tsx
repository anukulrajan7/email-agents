import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router';
import { afterEach, describe, expect, it } from 'vitest';
import { GuidePage } from './GuidePage';
import { GuideProvider } from './GuideProvider';
import { PageGuideCard } from './PageGuideCard';
import { GUIDES, guideForPage, visibleGuides } from './guides';
import { readDismissed } from './guideStorage';

const sources = import.meta.glob(['../**/*.tsx', '../../components/**/*.tsx', '../../app/**/*.tsx'], {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

afterEach(() => localStorage.clear());

function renderAt(path: string, user = 'u1', audience = { isAdmin: true, hasStaffing: true }) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <GuideProvider ws="w1" user={user} {...audience}>
        <Routes>
          <Route path="/w/:ws/guide" element={<GuidePage />} />
          <Route path="/w/:ws/:page" element={<PageGuideCard />} />
        </Routes>
      </GuideProvider>
    </MemoryRouter>,
  );
}

describe('guide content', () => {
  it('gives every guide a plain summary and 2 to 4 steps', () => {
    for (const g of GUIDES) {
      expect(g.what.length, g.id).toBeGreaterThan(10);
      expect(g.steps.length, g.id).toBeGreaterThanOrEqual(2);
      expect(g.steps.length, g.id).toBeLessThanOrEqual(4);
    }
    expect(new Set(GUIDES.map((g) => g.id)).size).toBe(GUIDES.length);
  });

  it('only routes to pages that exist', () => {
    const routes = sources['../../app/routes.tsx'] ?? '';
    for (const g of GUIDES) expect(routes, g.id).toContain(`path="${g.page}"`);
  });

  it('uses no model names', () => {
    expect(JSON.stringify(GUIDES)).not.toMatch(/gpt|claude|gemini|anthropic|openai|haiku|sonnet|opus/i);
  });

  it('has no tour, spotlight or anchor code left', () => {
    const files = Object.keys(sources).filter((p) => !p.includes('.test.'));
    expect(files.some((p) => /Spotlight|GuideLauncher/.test(p))).toBe(false);
    for (const p of files) expect(sources[p], p).not.toMatch(/data-guide|Spotlight|Show me around|Skip tour|GuideLauncher/);
  });
});

describe('pack and role filtering', () => {
  const ids = (a: { isAdmin: boolean; hasStaffing: boolean }) => visibleGuides(a).map((g) => g.id);

  it('hides admin-only guides from operators', () => {
    expect(ids({ isAdmin: false, hasStaffing: true })).not.toContain('packs');
    expect(ids({ isAdmin: false, hasStaffing: true })).not.toContain('usage');
    expect(ids({ isAdmin: true, hasStaffing: true })).toEqual(expect.arrayContaining(['packs', 'usage']));
  });

  it('hides HR guides unless the staffing pack is on', () => {
    expect(ids({ isAdmin: true, hasStaffing: false })).not.toContain('hr-workflows');
    expect(ids({ isAdmin: false, hasStaffing: true })).toEqual(expect.arrayContaining(['matching', 'hr-workflows']));
  });

  it('shows a page no card it cannot see', () => {
    expect(guideForPage('packs', { isAdmin: false, hasStaffing: true })).toBeUndefined();
    expect(guideForPage('matching', { isAdmin: true, hasStaffing: false })).toBeUndefined();
    expect(guideForPage('labels', { isAdmin: false, hasStaffing: false })?.id).toBe('labels');
  });
});

describe('page explanation card', () => {
  it('is one line until "How to use" is opened, which shows the steps and why it is safe', async () => {
    const user = userEvent.setup();
    renderAt('/w/w1/labels');
    const card = screen.getByRole('region', { name: /About this page: Labels/ });
    expect(within(card).getByText('What this page does')).toBeInTheDocument();
    expect(within(card).queryByText('How to use it')).not.toBeInTheDocument();
    const toggle = within(card).getByRole('button', { name: 'How to use' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    await user.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(within(card).getByText('How to use it')).toBeInTheDocument();
    expect(within(card).getByText('Why it is safe')).toBeInTheDocument();
    expect(within(card).getAllByRole('listitem').length).toBeGreaterThanOrEqual(2);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('is dismissed once, remembered for that person, and stays for someone else', async () => {
    const user = userEvent.setup();
    const first = renderAt('/w/w1/labels', 'u1');
    await user.click(screen.getByRole('button', { name: /Dismiss: Labels/ }));
    expect(screen.queryByRole('region', { name: /About this page/ })).not.toBeInTheDocument();
    expect(readDismissed('u1', 'w1').has('labels')).toBe(true);
    first.unmount();

    const again = renderAt('/w/w1/labels', 'u1');
    expect(screen.queryByRole('region', { name: /About this page/ })).not.toBeInTheDocument();
    again.unmount();

    renderAt('/w/w1/labels', 'u2');
    expect(screen.getByRole('region', { name: /About this page: Labels/ })).toBeInTheDocument();
  });

  it('shows nothing on a page without a guide', () => {
    renderAt('/w/w1/welcome');
    expect(screen.queryByRole('region')).not.toBeInTheDocument();
  });
});

describe('Guide page', () => {
  it('is a plain grid of the same cards with a link to each page and no walkthrough', async () => {
    renderAt('/w/w1/guide');
    const grid = screen.getAllByRole('article');
    expect(grid).toHaveLength(visibleGuides({ isAdmin: true, hasStaffing: true }).length);
    const labels = screen.getByTestId('guide-card-labels');
    expect(within(labels).getByRole('link', { name: /Go to page: Labels/ })).toHaveAttribute('href', '/w/w1/labels');
    expect(within(labels).getByText('Why it is safe.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Show me/ })).not.toBeInTheDocument();
    expect(screen.queryByText(/tried/i)).not.toBeInTheDocument();
  });

  it('brings dismissed page cards back', async () => {
    const user = userEvent.setup();
    localStorage.setItem('guide:v2:u1:w1:dismissed', JSON.stringify(['labels']));
    renderAt('/w/w1/guide');
    await user.click(screen.getByRole('button', { name: 'Show the page cards again' }));
    expect(readDismissed('u1', 'w1').size).toBe(0);
    expect(screen.queryByRole('button', { name: 'Show the page cards again' })).not.toBeInTheDocument();
  });
});
