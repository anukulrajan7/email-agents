// Matrix: AX-01 (create, toggle, delete, presets, admin-only), AX-03 (send needs a draft step), AX-05 (test on recent
// mail with no side effects, run history).
import { sentMail } from './support/api';
import { expect, signedInAs, test } from './support/test';

test.describe.configure({ mode: 'serial' });

const NAME = 'Review pricing questions';

test.describe('as an admin', () => {
  test.use(signedInAs('admin'));

  test('presets are on from the start and have already run on the seeded mail', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/automations`);
    const list = page.getByRole('list', { name: 'Automations' });
    await expect(list).toContainText('Reply to questions');
    await expect(list).toContainText('Chase important emails');
    await expect(page.getByRole('switch', { name: 'Reply to questions enabled' })).toBeChecked();
    await expect(list.getByRole('listitem').filter({ hasText: 'Reply to questions' })).toContainText(/Ran \d+ times?/);
  });

  test('the run history lists each email and what happened to each step', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/automations`);
    await page.getByRole('button', { name: 'Runs for Reply to questions' }).click();
    const drawer = page.getByRole('dialog', { name: 'Runs: Reply to questions' });
    const runs = drawer.getByRole('list', { name: 'Runs' });
    await expect(runs.getByRole('listitem').first()).toBeVisible();
    await expect(runs.getByRole('list', { name: 'Steps' }).first()).toContainText('Draft a reply');
  });

  test('building an automation: validation, a plain-words preview, save, test without side effects, toggle, delete', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/automations`);
    await page.getByRole('button', { name: 'New automation' }).click();

    await page.getByRole('button', { name: 'Create automation' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Fix these before saving' })).toBeVisible();

    await page.getByLabel('Name', { exact: true }).fill(NAME);
    await page.getByRole('button', { name: '+ Add a condition' }).click();
    await page.getByLabel('Condition 1 field').selectOption({ label: 'Subject' });
    await page.getByLabel('Condition 1 value').fill('pricing');
    await page.getByRole('group', { name: 'Add a step' }).getByRole('button', { name: '+ Create a review task' }).click();
    const preview = page.getByRole('complementary', { name: 'Preview' });
    await expect(preview).toContainText('Ready to save.');
    await expect(preview).toContainText('pricing');
    await page.getByRole('button', { name: 'Create automation' }).click();

    const card = page.getByRole('list', { name: 'Automations' }).getByRole('listitem').filter({ hasText: NAME });
    await expect(card).toBeVisible();
    await expect(card.getByRole('switch', { name: `${NAME} enabled` })).toBeChecked();

    const sentBefore = (await sentMail()).length;
    await card.getByRole('button', { name: `Edit ${NAME}` }).click();
    await page.getByRole('button', { name: 'Test on recent emails' }).click();
    const result = page.getByRole('status').filter({ hasText: 'would have matched' });
    await expect(result).toContainText('1 of your last 50 emails would have matched');
    await expect(result).toContainText('Question about pricing');
    expect((await sentMail()).length).toBe(sentBefore);
    await page.getByRole('button', { name: 'Cancel' }).click();

    await card.getByRole('switch', { name: `${NAME} enabled` }).click();
    await expect(card.getByRole('switch', { name: `${NAME} enabled` })).not.toBeChecked();
    await page.reload();
    await expect(
      page.getByRole('list', { name: 'Automations' }).getByRole('switch', { name: `${NAME} enabled` }),
    ).not.toBeChecked();

    await page.getByRole('button', { name: `Delete ${NAME}` }).click();
    await page.getByRole('dialog').getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(page.getByRole('list', { name: 'Automations' })).not.toContainText(NAME);
  });

  test('"Send the reply" without a draft step is flagged and cannot be saved', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/automations`);
    await page.getByRole('button', { name: 'New automation' }).click();
    await page.getByLabel('Name', { exact: true }).fill('Send without draft');
    await page.getByRole('group', { name: 'Add a step' }).getByRole('button', { name: '+ Send the reply' }).click();
    await expect(page.getByText('Add “Draft a reply” before this step.')).toBeVisible();
    await expect(page.getByText('Sends automatically.')).toBeVisible();
    await page.getByRole('button', { name: 'Create automation' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Fix these before saving' })).toBeVisible();
  });
});

test.describe('as an operator', () => {
  test.use(signedInAs('operator'));

  test('can see automations but not create, edit or switch them', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/automations`);
    await expect(page.getByRole('list', { name: 'Automations' })).toContainText('Reply to questions');
    await expect(page.getByRole('button', { name: 'New automation' })).toHaveCount(0);
    await expect(page.getByRole('switch', { name: 'Reply to questions enabled' })).toBeDisabled();
    await expect(page.getByRole('button', { name: /^Edit / })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Delete / })).toHaveCount(0);
  });
});
