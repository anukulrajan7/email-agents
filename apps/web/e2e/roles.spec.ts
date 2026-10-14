// Matrix: RL-01 (sidebar by role), RL-02 (admin endpoints refuse operators), PK-02. Manager and super-admin roles
// do not exist on main yet: see the fixme at the bottom.
import { call } from './support/api';
import { expect, sidebar, signedInAs, test } from './support/test';

const ADMIN_ONLY = ['Packs', 'Jobs', 'AI usage'];
const EVERYONE = ['Inbox', 'Agents', 'Alerts', 'Labels', 'Memory', 'Automations', 'Integrations', 'Reviews'];

test.describe('admin', () => {
  test.use(signedInAs('admin'));

  test('sees every page in the sidebar, including Packs, Jobs and AI usage', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    for (const name of [...EVERYONE, ...ADMIN_ONLY]) await expect(sidebar(page).getByRole('link', { name })).toBeVisible();
  });

  test('the workspace footer says who is signed in and offers Sign out', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await expect(page.getByText('Alice Admin')).toBeVisible();
    await expect(page.getByText('alice@e2e.test')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sign out' })).toBeVisible();
  });
});

test.describe('operator', () => {
  test.use(signedInAs('operator'));

  test('sees the work pages but not Packs, Jobs or AI usage', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    for (const name of EVERYONE) await expect(sidebar(page).getByRole('link', { name })).toBeVisible();
    for (const name of ADMIN_ONLY) await expect(sidebar(page).getByRole('link', { name })).toHaveCount(0);
  });

  for (const route of ['usage', 'jobs']) {
    test(`opening /${route} by URL sends an operator back to the inbox`, async ({ page, seed }) => {
      await page.goto(`/w/${seed.workspaceId}/${route}`);
      await expect(page).toHaveURL(new RegExp(`/w/${seed.workspaceId}/inbox$`));
    });
  }

  test('Integrations shows the sources but no Sync, Disconnect or test-email buttons', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/integrations`);
    await expect(page.getByText('Connect Gmail')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Sync now' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Disconnect' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Send test email' })).toHaveCount(0);
  });

  test('the server refuses admin actions even when the page is bypassed', async ({ seed }) => {
    const ws = `/workspaces/${seed.workspaceId}`;
    const token = seed.operator.token;
    const attempts: [string, string, unknown?][] = [
      ['PUT', `${ws}/packs`, { packs: ['general', 'staffing'] }],
      ['PUT', `${ws}/reply-settings`, { auto_reply: true, reply_instructions: '', signature: null }],
      ['GET', `${ws}/ai-usage`],
      ['GET', `${ws}/jobs?status=dead`],
      ['POST', `${ws}/automations`, { name: 'x', enabled: true, conditions: [], actions: [{ type: 'create_review' }] }],
      ['POST', `${ws}/sources/forwarding`],
      ['POST', `${ws}/labels`, { name: 'Sneaky', description: 'An operator tries to add a label.' }],
    ];
    for (const [method, path, body] of attempts) {
      const res = await call(path, { method, token, body });
      expect(res.status, `${method} ${path}`).toBe(403);
    }
  });
});

test.describe('signed out', () => {
  test('a workspace URL sends a visitor to the sign-in page', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await expect(page).toHaveURL(/\/sign-in$/);
    await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  });
});

// RL-04..RL-06: manager and super-admin roles (HRMS people directory) are not on main yet.
test.fixme('RL-04 a super admin manages members, roles, packs and the people directory; every action is audited', async () => {});
test.fixme('RL-05 an admin or manager cannot take super-admin actions; a manager sees only their reports', async () => {});
test.fixme('RL-06 demoting the last super admin is refused', async () => {});
