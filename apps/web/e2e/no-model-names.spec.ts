// Matrix: SE-11 (no model name in rendered text anywhere). Visits every route in `apps/web/src/app/routes.tsx` for an
// admin, an operator and a staffing workspace, with real classifications, drafts and attachment readings on screen,
// and fails if the DOM (text, attributes, titles) mentions a model or provider.
import type { Page } from '@playwright/test';
import { call, createWorkspace, seed } from './support/api';
import { expect, MODEL_NAMES, signedInAs, test } from './support/test';

async function domWithoutScripts(page: Page): Promise<string> {
  return page.evaluate(() => {
    const clone = document.documentElement.cloneNode(true) as HTMLElement;
    clone.querySelectorAll('script, style').forEach((n) => n.remove());
    return `${clone.innerText ?? ''}\n${clone.outerHTML}`;
  });
}

async function expectClean(page: Page, where: string) {
  await page.waitForLoadState('networkidle');
  const dom = await domWithoutScripts(page);
  const hit = MODEL_NAMES.exec(dom);
  expect(hit?.[0], `${where} shows a model or provider name`).toBeUndefined();
}

const ROUTES = [
  'inbox',
  'agents',
  'labels',
  'reviews',
  'alerts',
  'memory',
  'automations',
  'integrations',
  'packs',
  'usage',
  'jobs',
];

test.describe('admin, General workspace with mail, drafts and attachments', () => {
  test.use(signedInAs('admin'));

  for (const route of ROUTES) {
    test(`/${route}`, async ({ page, seed }) => {
      await page.goto(`/w/${seed.workspaceId}/${route}`);
      await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible();
      await expectClean(page, `/${route}`);
    });
  }

  test('every seeded email, including its insight, draft, classification and attachment readings', async ({ page, seed }) => {
    for (const key of ['java', 'timesheet']) {
      await call(`/workspaces/${seed.workspaceId}/messages/${seed.messages[key]}/draft`, { token: seed.admin.token, method: 'POST' });
      await expect
        .poll(async () => (await call<{ draft: unknown }>(`/workspaces/${seed.workspaceId}/messages/${seed.messages[key]}`, { token: seed.admin.token })).body.draft, {
          timeout: 20_000,
        })
        .toBeTruthy();
    }
    for (const [key, id] of Object.entries(seed.messages)) {
      await page.goto(`/w/${seed.workspaceId}/inbox/${id}`);
      await expect(page.getByRole('article', { name: 'Email' })).toBeVisible();
      for (const toggle of await page.getByRole('button', { name: 'Show preview' }).all()) await toggle.click();
      await expectClean(page, `message ${key}`);
    }
  });

  test('contact pages, alert settings, label health and the assistant panel', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/memory/contacts/${encodeURIComponent('priya.sharma@clientco.example')}`);
    await expectClean(page, 'contact page');

    await page.goto(`/w/${seed.workspaceId}/alerts`);
    await page.getByRole('tab', { name: 'Settings' }).click();
    await expectClean(page, 'alert settings');

    await page.goto(`/w/${seed.workspaceId}/labels`);
    await page.getByRole('tab', { name: /Health/ }).click();
    await expectClean(page, 'label health');

    await page.goto(`/w/${seed.workspaceId}/agents`);
    await page.getByRole('button', { name: /^Edit Meera/ }).click();
    await expectClean(page, 'agent editor');
  });

  test('the Ask Meera panel after an answer', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await page.getByRole('button', { name: 'Ask Meera' }).click();
    await page.getByLabel('Message to Meera').fill('Capgemini');
    await page.getByRole('dialog', { name: 'Ask Meera' }).getByRole('button', { name: 'Send' }).click();
    await expect(page.getByRole('dialog', { name: 'Ask Meera' }).getByText('Sources')).toBeVisible({ timeout: 20_000 });
    await expectClean(page, 'assistant panel');
  });

  test('the workspace picker and sign-in page', async ({ page }) => {
    await page.goto('/workspaces');
    await expect(page.getByRole('heading', { name: 'Choose a workspace' })).toBeVisible();
    await expectClean(page, '/workspaces');
  });
});

test.describe('operator', () => {
  test.use(signedInAs('operator'));

  for (const route of ['inbox', 'agents', 'labels', 'alerts', 'memory', 'automations', 'integrations', 'packs']) {
    test(`/${route}`, async ({ page, seed }) => {
      await page.goto(`/w/${seed.workspaceId}/${route}`);
      await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible();
      await expectClean(page, `operator /${route}`);
    });
  }
});

test.describe('HR & recruiting workspace', () => {
  test.use(signedInAs('admin'));

  test('matching, workflows and the rest', async ({ page }) => {
    const ws = await createWorkspace(seed().admin, `Names Lab ${Date.now() % 100000}`, ['staffing']);
    for (const route of ['inbox', 'matching', 'workflows', 'labels', 'agents', 'automations', 'packs']) {
      await page.goto(`/w/${ws}/${route}`);
      await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible();
      await expectClean(page, `staffing /${route}`);
    }
  });
});

test.describe('signed out', () => {
  for (const path of ['/sign-in', '/sign-up']) {
    test(path, async ({ page }) => {
      await page.goto(path);
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      await expectClean(page, path);
    });
  }
});
