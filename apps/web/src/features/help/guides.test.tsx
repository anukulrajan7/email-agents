import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { GuideLauncher } from './GuideLauncher';
import { GuidePage } from './GuidePage';
import { GuideProvider, useGuide } from './GuideProvider';
import { GUIDES, nudgeGuideFor, startHereGuides, visibleGuides } from './guides';
import { readNudged, readTried } from './guideStorage';
import { markTourDone } from './tourStorage';

const sources = import.meta.glob(['../**/*.tsx', '../../components/**/*.tsx', '../../app/**/*.tsx'], {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const PAGE_DIRS: Record<string, string[]> = {
  inbox: ['inbox', 'drafts', 'assistant'],
  labels: ['labels'],
  'agent-team': ['agents-hub'],
  agents: ['agents'],
  memory: ['memory'],
  automations: ['automations'],
  alerts: ['alerts'],
  packs: ['packs'],
  matching: ['matching'],
  people: ['team'],
  performance: ['team'],
  escalations: ['team'],
  timesheets: ['hrms'],
  portal: ['hrms'],
  workflows: ['workflows'],
  usage: ['usage'],
};

const anchorIn = (dirs: string[], anchor: string) =>
  Object.entries(sources).some(
    ([path, text]) =>
      !path.includes('.test.') &&
      dirs.some((d) => path.startsWith(`../${d}/`)) &&
      (text.includes(`data-guide="${anchor}"`) || text.includes(` guide="${anchor}"`)),
  );

describe('guide content', () => {
  it('gives every guide a plain summary and 2 to 4 steps', () => {
    for (const g of GUIDES) {
      expect(g.what.length, g.id).toBeGreaterThan(10);
      expect(g.steps.length, g.id).toBeGreaterThanOrEqual(2);
      expect(g.steps.length, g.id).toBeLessThanOrEqual(4);
    }
    expect(new Set(GUIDES.map((g) => g.id)).size).toBe(GUIDES.length);
  });

  it('points every guide and step at an anchor that exists in the real page component', () => {
    for (const g of GUIDES) {
      const dirs = PAGE_DIRS[g.page];
      expect(dirs, `${g.id} page`).toBeDefined();
      const anchors = new Set([g.anchor, ...g.steps.map((s) => s.anchor ?? g.anchor)]);
      for (const a of anchors) expect(anchorIn(dirs ?? [], a), `${g.id}: ${a}`).toBe(true);
    }
  });

  it('only routes to pages that exist', () => {
    const routes = sources['../../app/routes.tsx'] ?? '';
    for (const g of GUIDES) expect(routes, g.id).toContain(`path="${g.page}"`);
  });

  it('uses no model names', () => {
    expect(JSON.stringify(GUIDES)).not.toMatch(/gpt|claude|gemini|anthropic|openai|haiku|sonnet|opus/i);
  });

  it('has exactly three start-here guides for everyone', () => {
    expect(startHereGuides({ isAdmin: false, hasStaffing: false })).toHaveLength(3);
    expect(startHereGuides({ isAdmin: true, hasStaffing: true })).toHaveLength(3);
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
    expect(ids({ isAdmin: true, hasStaffing: false })).not.toEqual(expect.arrayContaining(['matching']));
    expect(ids({ isAdmin: true, hasStaffing: false })).not.toContain('hr-workflows');
    expect(ids({ isAdmin: false, hasStaffing: true })).toEqual(expect.arrayContaining(['matching', 'hr-workflows']));
  });

  it('does not nudge toward a guide the person cannot see', () => {
    expect(nudgeGuideFor('packs', { isAdmin: false, hasStaffing: true })).toBeUndefined();
    expect(nudgeGuideFor('matching', { isAdmin: true, hasStaffing: false })).toBeUndefined();
    expect(nudgeGuideFor('labels', { isAdmin: false, hasStaffing: false })?.id).toBe('labels');
  });
});

const RECT = { top: 120, left: 80, width: 320, height: 200, right: 400, bottom: 320, x: 80, y: 120, toJSON: () => ({}) };
const ZERO = { ...RECT, top: 0, left: 0, width: 0, height: 0, right: 0, bottom: 0, x: 0, y: 0 };

function Where() {
  return <p data-testid="where">{useLocation().pathname}</p>;
}

function StartButton({ id }: { id: string }) {
  const { start } = useGuide();
  return <button onClick={() => start(id)}>Open guide {id}</button>;
}

function renderGuide({
  withAnchor = true,
  isAdmin = true,
  hasStaffing = true,
  user = 'u1',
  ws = 'w1',
  path = '/w/w1/labels',
  launcher = false,
  children,
}: {
  withAnchor?: boolean;
  isAdmin?: boolean;
  hasStaffing?: boolean;
  user?: string;
  ws?: string;
  path?: string;
  launcher?: boolean;
  children?: React.ReactNode;
} = {}) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <GuideProvider ws={ws} user={user} isAdmin={isAdmin} hasStaffing={hasStaffing}>
        <Routes>
          <Route
            path="/w/:ws/:page/*"
            element={
              <main>
                <StartButton id="labels" />
                <Where />
                {withAnchor && <ul aria-label="Labels" data-guide="labels-list" />}
                {children}
              </main>
            }
          />
        </Routes>
        {launcher && <GuideLauncher />}
      </GuideProvider>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    return (this.hasAttribute('data-guide') ? RECT : ZERO) as DOMRect;
  });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  localStorage.clear();
});

describe('Spotlight walkthrough', () => {
  it('highlights the real element and walks Next, Back and Done from the keyboard', async () => {
    const user = userEvent.setup();
    renderGuide();
    const opener = screen.getByRole('button', { name: 'Open guide labels' });
    await user.click(opener);

    const dialog = await screen.findByRole('dialog', { name: 'Labels' });
    await waitFor(() => expect(screen.getByTestId('spotlight')).toHaveAttribute('data-target', 'found'));
    expect(screen.getByTestId('spotlight-cutout')).toBeInTheDocument();
    expect(dialog).toHaveTextContent('Step 1 of 3');
    await waitFor(() => expect(dialog).toHaveFocus());
    expect(screen.getByRole('status')).toHaveTextContent('Step 1 of 3');

    await user.keyboard('{ArrowRight}');
    expect(dialog).toHaveTextContent('Step 2 of 3');
    expect(screen.getByRole('status')).toHaveTextContent('Step 2 of 3');
    await user.keyboard('{ArrowLeft}');
    expect(dialog).toHaveTextContent('Step 1 of 3');

    await user.click(within(dialog).getByRole('button', { name: 'Next' }));
    await user.click(within(dialog).getByRole('button', { name: 'Back' }));
    await user.click(within(dialog).getByRole('button', { name: 'Next' }));
    await user.click(within(dialog).getByRole('button', { name: 'Next' }));
    expect(dialog).toHaveTextContent('Why it is safe');
    await user.click(within(dialog).getByRole('button', { name: 'Done' }));

    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
    expect(readTried('u1', 'w1').has('labels')).toBe(true);
  });

  it('keeps focus inside the callout when tabbing', async () => {
    const user = userEvent.setup();
    renderGuide();
    await user.click(screen.getByRole('button', { name: 'Open guide labels' }));
    const dialog = await screen.findByRole('dialog', { name: 'Labels' });
    await user.tab();
    expect(dialog).toContainElement(document.activeElement as HTMLElement);
    for (let i = 0; i < 6; i++) {
      await user.tab();
      expect(dialog).toContainElement(document.activeElement as HTMLElement);
    }
    await user.tab({ shift: true });
    expect(dialog).toContainElement(document.activeElement as HTMLElement);
  });

  it('closes on Escape without counting the guide as tried, and returns focus', async () => {
    const user = userEvent.setup();
    renderGuide();
    const opener = screen.getByRole('button', { name: 'Open guide labels' });
    await user.click(opener);
    await screen.findByRole('dialog', { name: 'Labels' });
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(opener).toHaveFocus();
    expect(readTried('u1', 'w1').size).toBe(0);
  });

  it('falls back to a centered card when the anchor is not on screen', async () => {
    const user = userEvent.setup();
    renderGuide({ withAnchor: false });
    await user.click(screen.getByRole('button', { name: 'Open guide labels' }));
    const dialog = await screen.findByRole('dialog', { name: 'Labels' });
    await waitFor(() => expect(screen.getByTestId('spotlight')).toHaveAttribute('data-target', 'missing'), { timeout: 3000 });
    expect(screen.queryByTestId('spotlight-cutout')).not.toBeInTheDocument();
    expect(dialog).toHaveTextContent('cannot point at this part');
    expect(dialog).toHaveStyle({ left: '50%', top: '50%' });
  });

  it('navigates to the guide page first when you are somewhere else', async () => {
    const user = userEvent.setup();
    renderGuide({
      path: '/w/w1/memory',
      children: <p>memory page</p>,
    });
    await user.click(screen.getByRole('button', { name: 'Open guide labels' }));
    await screen.findByRole('dialog', { name: 'Labels' });
    expect(screen.getByTestId('where')).toHaveTextContent('/w/w1/labels');
    await waitFor(() => expect(screen.getByTestId('spotlight')).toHaveAttribute('data-target', 'found'));
  });

  it('is still when the person prefers reduced motion', async () => {
    vi.stubGlobal('matchMedia', (query: string) => ({
      matches: query.includes('prefers-reduced-motion'),
      media: query,
      addEventListener: () => undefined,
      removeEventListener: () => undefined,
    }));
    const user = userEvent.setup();
    renderGuide();
    await user.click(screen.getByRole('button', { name: 'Open guide labels' }));
    const spot = await screen.findByTestId('spotlight');
    expect(spot).toHaveAttribute('data-motion', 'still');
    await waitFor(() => expect(screen.getByTestId('spotlight-cutout').className).not.toMatch(/transition/));
  });

  it('moves with motion when reduced motion is not requested', async () => {
    const user = userEvent.setup();
    renderGuide();
    await user.click(screen.getByRole('button', { name: 'Open guide labels' }));
    expect(await screen.findByTestId('spotlight')).toHaveAttribute('data-motion', 'smooth');
  });
});

describe('progress', () => {
  it('is saved per person and workspace', async () => {
    const user = userEvent.setup();
    const { unmount } = renderGuide({ launcher: false, children: <GuidePage /> });
    await user.click(screen.getByRole('button', { name: 'Labels: not tried yet' }));
    expect(screen.getByRole('button', { name: 'Labels: tried' })).toHaveAttribute('aria-pressed', 'true');
    expect(readTried('u1', 'w1')).toEqual(new Set(['labels']));
    expect(readTried('u2', 'w1').size).toBe(0);
    expect(readTried('u1', 'w2').size).toBe(0);
    unmount();

    renderGuide({ children: <GuidePage /> });
    expect(screen.getByRole('button', { name: 'Labels: tried' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Labels: tried' }));
    expect(readTried('u1', 'w1').size).toBe(0);
  });
});

describe('Guide hub', () => {
  it('shows Start here first and only the guides this person can see', () => {
    renderGuide({ isAdmin: false, hasStaffing: false, children: <GuidePage /> });
    const start = screen.getByRole('region', { name: 'Start here' });
    expect(within(start).getAllByRole('article')).toHaveLength(3);
    expect(screen.queryByText('Packs')).not.toBeInTheDocument();
    expect(screen.queryByText('AI usage')).not.toBeInTheDocument();
    expect(screen.queryByText('Matching')).not.toBeInTheDocument();
    expect(screen.getByRole('status', { name: '' })).toHaveTextContent(/0 of \d+ tried/);
    expect(screen.getAllByTestId('agent-face').length).toBeGreaterThan(3);
  });

  it('starts a guide from its Show me button', async () => {
    const user = userEvent.setup();
    renderGuide({ children: <GuidePage /> });
    await user.click(screen.getByRole('button', { name: 'Show me: Labels' }));
    expect(await screen.findByRole('dialog', { name: 'Labels' })).toBeInTheDocument();
  });
});

describe('welcome card and nudges', () => {
  it('shows the welcome card first, never together with a nudge, and remembers skipping it', async () => {
    const user = userEvent.setup();
    const { unmount } = renderGuide({ launcher: true });
    expect(screen.getByRole('region', { name: 'Quick tour' })).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: 'New here?' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Skip tour' }));
    expect(screen.queryByRole('region', { name: 'Quick tour' })).not.toBeInTheDocument();
    expect(localStorage.getItem('tour:v1:done')).toBe('1');
    unmount();
    renderGuide({ launcher: true });
    expect(screen.queryByRole('region', { name: 'Quick tour' })).not.toBeInTheDocument();
  });

  it('the welcome card walks the three start-here guides in a row', async () => {
    const user = userEvent.setup();
    renderGuide({ path: '/w/w1/inbox', launcher: true });
    await user.click(screen.getByRole('button', { name: 'Show me around' }));
    expect(await screen.findByRole('dialog', { name: 'Inbox and reader' })).toBeInTheDocument();
    await user.keyboard('{ArrowRight}');
    await user.click(screen.getByRole('button', { name: 'Next guide' }));
    expect(await screen.findByRole('dialog', { name: 'Replies, drafts and undo send' })).toBeInTheDocument();
  });

  it('nudges once on a first visit, opens that guide, and stays quiet afterwards', async () => {
    markTourDone();
    const user = userEvent.setup();
    const { unmount } = renderGuide({ launcher: true });
    const nudge = await screen.findByRole('region', { name: 'New here?' });
    expect(nudge).toHaveTextContent('See how labels works');
    await user.click(within(nudge).getByRole('button', { name: 'Show me' }));
    expect(await screen.findByRole('dialog', { name: 'Labels' })).toBeInTheDocument();
    expect(readNudged('u1', 'w1').has('labels')).toBe(true);
    await act(async () => {
      await user.keyboard('{Escape}');
    });
    expect(screen.queryByRole('region', { name: 'New here?' })).not.toBeInTheDocument();
    unmount();
    renderGuide({ launcher: true });
    expect(screen.queryByRole('region', { name: 'New here?' })).not.toBeInTheDocument();
  });

  it('can be dismissed with Not now', async () => {
    markTourDone();
    const user = userEvent.setup();
    renderGuide({ launcher: true });
    await screen.findByRole('region', { name: 'New here?' });
    await user.click(screen.getByRole('button', { name: 'Not now' }));
    expect(screen.queryByRole('region', { name: 'New here?' })).not.toBeInTheDocument();
  });

  it('does not nudge a guide that is already tried', () => {
    markTourDone();
    localStorage.setItem('guide:v1:u1:w1:tried', JSON.stringify(['labels']));
    renderGuide({ launcher: true });
    expect(screen.queryByRole('region', { name: 'New here?' })).not.toBeInTheDocument();
  });
});
