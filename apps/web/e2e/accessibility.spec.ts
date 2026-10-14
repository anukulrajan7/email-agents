// Accessibility smoke (WCAG 2.1 A/AA, axe-core): no serious or critical violations on the key pages.
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { expect, signedInAs, signedOut, test } from './support/test';

async function violations(page: Page) {
  const results = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  return results.violations
    .filter((v) => v.impact === 'serious' || v.impact === 'critical')
    .map((v) => `${v.impact} ${v.id}: ${v.help} (${v.nodes.length}) e.g. ${v.nodes[0]?.target.join(' ')}`);
}

async function settled(page: Page) {
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(400);
}

test.describe('signed out', () => {
  test.use(signedOut);

  for (const path of ['/sign-in', '/sign-up']) {
    test(`${path} has no serious violations`, async ({ page }) => {
      await page.goto(path);
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      await settled(page);
      expect(await violations(page)).toEqual([]);
    });
  }
});

test.describe('signed in as an admin', () => {
  test.use(signedInAs('admin'));

  test('the workspace picker', async ({ page }) => {
    await page.goto('/workspaces');
    await expect(page.getByRole('heading', { name: 'Choose a workspace' })).toBeVisible();
    await settled(page);
    expect(await violations(page)).toEqual([]);
  });

  for (const route of ['inbox', 'labels', 'alerts', 'automations', 'memory', 'agents', 'integrations', 'packs', 'usage']) {
    test(`/${route}`, async ({ page, seed }) => {
      await page.goto(`/w/${seed.workspaceId}/${route}`);
      await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible();
      await settled(page);
      expect(await violations(page)).toEqual([]);
    });
  }

  test('an open email with attachments', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox/${seed.messages.timesheet}`);
    await expect(page.getByRole('article', { name: 'Email' })).toBeVisible();
    await page.getByRole('button', { name: 'Show preview' }).click();
    await settled(page);
    expect(await violations(page)).toEqual([]);
  });

  test('the Ask Meera panel', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await expect(page.getByRole('button', { name: 'Ask Meera' })).toBeVisible();
    await page.keyboard.press('Control+k');
    await expect(page.getByRole('dialog', { name: 'Ask Meera' })).toBeVisible();
    await settled(page);
    expect(await violations(page)).toEqual([]);
  });
});
