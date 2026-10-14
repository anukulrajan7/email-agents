// Matrix: AL-01 (unanswered important mail raises an alert, emailed to the team), AL-02 (resolve, snooze, reopen,
// edit settings, send a test). The seeded urgent mail is 28 hours old and unanswered; the worker raises the alert on
// its next maintenance tick (every 30 s), so the first test waits for it.
import { call, mailTo } from './support/api';
import { expect, signedInAs, test } from './support/test';

test.describe.configure({ mode: 'serial' });

test.describe('as an admin', () => {
  test.use(signedInAs('admin'));

  test.beforeEach(async ({ seed }) => {
    await expect
      .poll(
        async () => {
          const res = await call<{ items: unknown[] }>(`/workspaces/${seed.workspaceId}/alerts`, { token: seed.admin.token });
          return res.body.items.length;
        },
        { timeout: 75_000, intervals: [1_000] },
      )
      .toBeGreaterThan(0);
  });

  test('an unanswered urgent email raises a high alert that links to the email and says who was told', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/alerts`);
    const card = page.getByRole('group', { name: /No reply yet: URGENT: production outage/ });
    await expect(card).toBeVisible();
    await expect(card).toContainText('High');
    await expect(card).toContainText('nobody has replied');
    await expect(card).toContainText('Notified: alice@e2e.test');
    await card.getByRole('link', { name: 'URGENT: production outage needs your reply' }).click();
    await expect(page).toHaveURL(new RegExp(`/inbox/${seed.messages.urgent}$`));
    await expect(page.getByRole('article', { name: 'Email' })).toContainText('production system is down');
  });

  test('the team was emailed about the alert', async ({ seed }) => {
    await expect.poll(() => mailTo(seed.admin.email).filter((m) => /No reply yet/i.test(m.subject + m.text)).length, { timeout: 30_000 }).toBeGreaterThan(0);
  });

  test('snooze moves an alert aside and Reopen brings it back', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/alerts`);
    const title = 'No reply yet: Invoice INV-1042 for October services';
    const card = page.getByRole('group', { name: title });
    await page.getByLabel(`Snooze: ${title}`).selectOption({ label: '4 hours' });
    await expect(card).toContainText('Snoozed');
    await card.getByRole('button', { name: 'Reopen' }).click();
    await expect(card).not.toContainText('Snoozed');
  });

  test('resolving an alert takes it off Open and lists it under Resolved', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/alerts`);
    const title = 'No reply yet: Question about pricing';
    await page.getByRole('group', { name: title }).getByRole('button', { name: 'Resolve' }).click();
    await expect(page.getByRole('group', { name: title })).toHaveCount(0);
    await page.getByRole('tab', { name: /Resolved/ }).click();
    await expect(page.getByRole('group', { name: title })).toContainText('Resolved');
  });

  test('settings validate, save, survive a reload and can send a test alert', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/alerts`);
    await page.getByRole('tab', { name: 'Settings' }).click();
    const hours = page.getByLabel('Hours until alert for an unanswered important email');
    await expect(hours).toHaveValue('4');
    await hours.fill('0');
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(hours).toHaveAttribute('aria-invalid', 'true');
    await expect(page.getByRole('alert').first()).toBeVisible();
    await hours.fill('6');
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(page.getByRole('status').filter({ hasText: /Saved\./ })).toBeVisible();
    await page.reload();
    await page.getByRole('tab', { name: 'Settings' }).click();
    await expect(page.getByLabel('Hours until alert for an unanswered important email')).toHaveValue('6');
    const before = mailTo(seed.admin.email).length;
    await page.getByRole('button', { name: 'Send test alert' }).click();
    await expect(page.getByText(/Sent to alice@e2e.test/)).toBeVisible();
    await expect.poll(() => mailTo(seed.admin.email).length).toBeGreaterThan(before);
    await page.getByLabel('Hours until alert for an unanswered important email').fill('4');
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(page.getByRole('status').filter({ hasText: /Saved\./ })).toBeVisible();
  });
});

test.describe('as an operator', () => {
  test.use(signedInAs('operator'));

  test('can act on alerts but has no Settings tab', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/alerts`);
    await expect(page.getByRole('tab', { name: /Open/ })).toBeVisible();
    await expect(page.getByRole('tab', { name: 'Settings' })).toHaveCount(0);
  });
});
