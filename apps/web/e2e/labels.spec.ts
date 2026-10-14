// Matrix: LB-01 (custom label), LB-02 (system label protected, reorder), LB-03 (one-click correction saved as manual),
// LB-04/LB-05 are API-level. Admin-only editing is checked as an operator.
import { expect, openMessage, signedInAs, test } from './support/test';

test.describe.configure({ mode: 'serial' });

const labelRow = (page: import('@playwright/test').Page, name: string) =>
  page.getByRole('list', { name: 'Labels' }).getByRole('listitem').filter({ has: page.getByRole('button', { name: `Edit ${name}` }) });

test.describe('as an admin', () => {
  test.use(signedInAs('admin'));

  test('the General pack brings its labels with plain descriptions and counts, and no HR labels', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/labels`);
    const list = page.getByRole('list', { name: 'Labels' });
    for (const name of ['Action required', 'Question', 'Billing', 'Scheduling', 'Other']) {
      await expect(list).toContainText(name);
    }
    for (const hr of ['Timesheet', 'Candidate referral', 'Payroll', 'Client requirement']) {
      await expect(list).not.toContainText(hr);
    }
    await expect(labelRow(page, 'Billing')).toContainText('2 emails');
  });

  test('adds a custom label with a description, validates it, and can archive it again', async ({ page, seed }) => {
    const name = `Contract renewals ${Date.now() % 10000}`;
    await page.goto(`/w/${seed.workspaceId}/labels`);
    await page.getByLabel('Name', { exact: true }).fill(name);
    await page.getByLabel('Description').fill('short');
    await expect(page.getByRole('button', { name: 'Add label' })).toBeDisabled();
    await page.getByLabel('Description').fill('Renewal notices and contract extensions from our customers.');
    await page.getByRole('button', { name: 'Add label' }).click();
    await expect(labelRow(page, name)).toContainText('0 emails');
    await page.getByRole('button', { name: `Archive ${name}` }).click();
    await expect(labelRow(page, name)).toContainText('Archived');
  });

  test('the system label Other cannot be archived', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/labels`);
    await expect(page.getByRole('button', { name: 'Archive Other' })).toBeDisabled();
  });

  test('reordering a label is saved', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/labels`);
    const names = () =>
      page
        .getByRole('list', { name: 'Labels' })
        .getByRole('button', { name: /^Edit / })
        .evaluateAll((els) => els.map((e) => e.getAttribute('aria-label') ?? ''));
    const before = (await names()).map((n) => n.replace('Edit ', ''));
    await page.getByRole('button', { name: `Move ${before[0]} down` }).click();
    await expect.poll(async () => (await names())[0]).toBe(`Edit ${before[1]}`);
    await page.reload();
    expect((await names())[0]).toBe(`Edit ${before[1]}`);
    await page.getByRole('button', { name: `Move ${before[0]} up` }).click();
    await expect.poll(async () => (await names())[0]).toBe(`Edit ${before[0]}`);
  });

  test('a label change in one click is saved as the team’s choice and shows in the list and filter', async ({ page, seed }) => {
    await openMessage(page, seed, 'label1');
    const panel = page.getByLabel('Change label');
    await expect(panel).toHaveValue('billing');
    await panel.selectOption({ label: 'FYI' });
    await expect(page.getByRole('status').filter({ hasText: 'Saved' })).toBeVisible();
    await expect(page.getByText('Labelled FYI by you.')).toBeVisible();
    await page.reload();
    await expect(page.getByLabel('Change label')).toHaveValue('fyi');
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await page.getByLabel('Filter by label').selectOption({ label: 'FYI' });
    await expect(page.getByRole('list', { name: 'Messages in this view' })).toContainText('Invoice INV-1042');
    await page.goto(`/w/${seed.workspaceId}/inbox/${seed.messages.label1}`);
    await page.getByLabel('Change label').selectOption({ label: 'Billing' });
    await expect(page.getByText('Labelled Billing by you.')).toBeVisible();
  });
});

test.describe('as an operator', () => {
  test.use(signedInAs('operator'));

  test('can read labels but not add or change them', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/labels`);
    await expect(page.getByRole('list', { name: 'Labels' })).toContainText('Billing');
    await expect(page.getByText('Only workspace admins can add or change labels.')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Add label' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: /^Archive / })).toHaveCount(0);
  });

  test('can still correct the label on a mail', async ({ page, seed }) => {
    await openMessage(page, seed, 'newsletter');
    await expect(page.getByLabel('Change label')).toBeEnabled();
  });
});
