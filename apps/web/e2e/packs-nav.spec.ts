// Matrix: PK-01 (enable the HR pack, HR behaviour appears; disabling keeps data), PK-02 (operators are told it is
// admin-only), PK-04 (a General workspace hides Matching and Workflows). Runs in a throwaway workspace.
import { createWorkspace, seed } from './support/api';
import { expect, sidebar, signedInAs, test } from './support/test';

test.describe.configure({ mode: 'serial' });

let lab = '';

test.beforeAll(async () => {
  const s = seed();
  lab = await createWorkspace(s.admin, `Packs Lab ${Date.now() % 100000}`, ['general'], [s.operator]);
});

test.describe('as an admin', () => {
  test.use(signedInAs('admin'));

  test('a General workspace has no Matching or Workflows, and enabling HR & recruiting adds them and its labels', async ({ page }) => {
    await page.goto(`/w/${lab}/inbox`);
    const nav = sidebar(page);
    await expect(nav.getByRole('link', { name: 'Inbox' })).toBeVisible();
    await expect(nav.getByRole('link', { name: 'Matching' })).toHaveCount(0);
    await expect(nav.getByRole('link', { name: 'Workflows' })).toHaveCount(0);

    await nav.getByRole('link', { name: 'Packs' }).click();
    await expect(page.getByRole('checkbox', { name: /Business inbox/ })).toBeChecked();
    await expect(page.getByRole('checkbox', { name: /HR & recruiting/ })).not.toBeChecked();
    await page.getByRole('checkbox', { name: /HR & recruiting/ }).check();
    await page.getByRole('button', { name: 'Save packs' }).click();
    await expect(page.getByText('Packs updated.')).toBeVisible();

    await expect(nav.getByRole('link', { name: 'Matching' })).toBeVisible();
    await expect(nav.getByRole('link', { name: 'Workflows' })).toBeVisible();
    await nav.getByRole('link', { name: 'Labels' }).click();
    await expect(page.getByRole('list', { name: 'Labels' })).toContainText('Timesheet');
  });

  test('turning the HR pack off hides its pages again but keeps the labels', async ({ page }) => {
    await page.goto(`/w/${lab}/packs`);
    await page.getByRole('checkbox', { name: /HR & recruiting/ }).uncheck();
    await page.getByRole('button', { name: 'Save packs' }).click();
    await expect(page.getByText('Packs updated.')).toBeVisible();
    await expect(sidebar(page).getByRole('link', { name: 'Matching' })).toHaveCount(0);
    await expect(sidebar(page).getByRole('link', { name: 'Workflows' })).toHaveCount(0);
    await sidebar(page).getByRole('link', { name: 'Labels' }).click();
    await expect(page.getByRole('list', { name: 'Labels' })).toContainText('Timesheet');
  });

  test('a General workspace does not use HR wording on Integrations and Automations', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/integrations`);
    await expect(page.getByText('Connect Gmail')).toBeVisible();
    expect(await page.locator('main').innerText()).not.toMatch(/\bHR\b/);
    await page.goto(`/w/${seed.workspaceId}/automations`);
    await expect(page.getByRole('heading', { name: 'Automations' })).toBeVisible();
    expect(await page.locator('main').innerText()).not.toMatch(/\bHR\b/);
  });

  test('at least one pack must stay on', async ({ page }) => {
    await page.goto(`/w/${lab}/packs`);
    await page.getByRole('checkbox', { name: /Business inbox/ }).uncheck();
    await expect(page.getByRole('button', { name: 'Save packs' })).toBeDisabled();
  });
});

test.describe('as an operator', () => {
  test.use(signedInAs('operator'));

  test('has no Packs link and is told the page is for admins', async ({ page }) => {
    await page.goto(`/w/${lab}/inbox`);
    await expect(sidebar(page).getByRole('link', { name: 'Packs' })).toHaveCount(0);
    await page.goto(`/w/${lab}/packs`);
    await expect(page.getByText('Admins only')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Save packs' })).toHaveCount(0);
  });
});
