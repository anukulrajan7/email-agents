// Matrix: ME-01 (teach, pin, delete a fact), ME-02 (a contact's page: facts and history). LR-09 (preferences the agents
// learned) is not built: fixme at the bottom.
import { expect, openMessage, signedInAs, test } from './support/test';

test.use(signedInAs('operator'));
test.describe.configure({ mode: 'serial' });

const FACT = 'Our enterprise plan is billed yearly, per seat.';

test('teach a fact, see it under "Added by you", pin it, then delete it after a confirmation', async ({ page, seed }) => {
  await page.goto(`/w/${seed.workspaceId}/memory`);
  await page.getByText('Teach a fact').click();
  await page.getByLabel('Something the assistant should know').fill(FACT);
  await page.getByRole('button', { name: 'Add fact' }).click();

  await page.getByRole('tab', { name: /Added by you/ }).click();
  const item = page.getByRole('listitem').filter({ hasText: FACT });
  await expect(item).toBeVisible();
  await expect(item).toContainText('Added by you');

  await item.getByRole('button', { name: 'Pin' }).click();
  await expect(item).toContainText('Pinned');
  await page.reload();
  await page.getByRole('tab', { name: /Added by you/ }).click();
  await expect(page.getByRole('listitem').filter({ hasText: FACT })).toContainText('Pinned');

  await page.getByRole('listitem').filter({ hasText: FACT }).getByRole('button', { name: 'Delete' }).click();
  const dialog = page.getByRole('dialog', { name: 'Delete this fact?' });
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(page.getByText(FACT)).toBeVisible();
  await page.getByRole('listitem').filter({ hasText: FACT }).getByRole('button', { name: 'Delete' }).click();
  await page.getByRole('dialog', { name: 'Delete this fact?' }).getByRole('button', { name: 'Delete' }).click();
  await expect(page.getByText(FACT)).toHaveCount(0);
});

test('a note about a sender, written from their email, shows on their contact page with their history', async ({ page, seed }) => {
  await openMessage(page, seed, 'receipt');
  await page.getByText('Remember something about this sender').click();
  await page.getByLabel('What should we remember?').fill('Kabir is the client contact for travel expenses.');
  await page.getByRole('button', { name: 'Add note' }).click();
  await expect(page.getByText('Saved. The assistant will use this from now on.')).toBeVisible();

  await page.getByRole('link', { name: 'Open profile' }).click();
  await expect(page).toHaveURL(/\/memory\/contacts\//);
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Kabir Shah');
  await expect(page.getByText('kabir.shah@clientco.example').first()).toBeVisible();
  await expect(page.getByText('Kabir is the client contact for travel expenses.')).toBeVisible();
  const recent = page.getByRole('region', { name: 'Recent messages' }).or(page.getByText('Cafe receipt for reimbursement'));
  await expect(recent.first()).toBeVisible();
});

test('the contact table on the Memory page links to each person', async ({ page, seed }) => {
  await page.goto(`/w/${seed.workspaceId}/memory`);
  const row = page.getByRole('row').filter({ hasText: 'priya.sharma@clientco.example' });
  await expect(row).toContainText('Priya Sharma');
  await row.getByRole('link').first().click();
  await expect(page.getByRole('heading', { level: 1 })).toHaveText('Priya Sharma');
});

test.fixme('LR-09 "What your agents learned": own preferences and facts, accept / edit / archive', async () => {});
