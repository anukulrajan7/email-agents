// Matrix: SE-01, SE-02 (UI side). Two workspaces owned by different people; user B tampers with user A's URLs and
// must see a not-found screen, never a flash of A's data, and the server must refuse the raw calls too.
import type { Page } from '@playwright/test';
import { call } from './support/api';
import { expect, signedInAs, test } from './support/test';

const SECRETS = ['September timesheet', 'Java Developer requirement', 'priya.sharma', 'Question about pricing', 'Capgemini'];

function watchTenantResponses(page: Page, workspaceId: string) {
  const leaked: string[] = [];
  page.on('response', (res) => {
    if (res.url().includes(`/api/v1/workspaces/${workspaceId}`) && res.ok()) leaked.push(`${res.status()} ${res.url()}`);
  });
  return leaked;
}

test.describe('user B opens user A’s workspace by URL', () => {
  test.use(signedInAs('outsider'));

  for (const route of ['inbox', 'agents', 'labels', 'alerts', 'memory', 'automations', 'integrations', 'packs', 'usage', 'jobs']) {
    test(`/${route} shows "Workspace not found" and loads nothing from A`, async ({ page, seed }) => {
      const leaked = watchTenantResponses(page, seed.workspaceId);
      await page.goto(`/w/${seed.workspaceId}/${route}`);
      await expect(page.getByText('Workspace not found')).toBeVisible();
      await expect(page.getByText('You are not a member of this workspace.')).toBeVisible();
      await expect(page.getByRole('button', { name: 'Choose a workspace' })).toBeVisible();
      const html = await page.content();
      for (const secret of SECRETS) expect(html).not.toContain(secret);
      expect(leaked).toEqual([]);
    });
  }

  test('a message URL from A shows nothing of the email', async ({ page, seed }) => {
    const leaked = watchTenantResponses(page, seed.workspaceId);
    await page.goto(`/w/${seed.workspaceId}/inbox/${seed.messages.timesheet}`);
    await expect(page.getByText('Workspace not found')).toBeVisible();
    expect(await page.content()).not.toContain('timesheet');
    expect(leaked).toEqual([]);
  });

  test('"Choose a workspace" leads to B’s own workspace only', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await page.getByRole('button', { name: 'Choose a workspace' }).click();
    await expect(page).toHaveURL(/\/workspaces$/);
    const list = page.getByRole('list', { name: 'Workspaces' });
    await expect(list).toContainText('Other Company');
    await expect(list).not.toContainText('Acme Desk');
  });

  test('B’s own workspace opens, is empty, and does not contain A’s mail', async ({ page, seed }) => {
    await page.goto(`/w/${seed.outsiderWorkspaceId}/inbox`);
    await expect(page.getByText('No messages yet')).toBeVisible();
    for (const secret of SECRETS) await expect(page.getByText(secret)).toHaveCount(0);
  });

  test('the server refuses B’s direct calls with A’s ids, and A’s ids under B’s path find nothing', async ({ seed }) => {
    const token = seed.outsider.token;
    const a = `/workspaces/${seed.workspaceId}`;
    for (const path of [
      `${a}/messages`,
      `${a}/messages/${seed.messages.timesheet}`,
      `${a}/search?q=Capgemini`,
      `${a}/memories`,
      `${a}/labels`,
      `${a}/alerts`,
      `${a}/cases`,
      `${a}/approvals`,
    ]) {
      const res = await call(path, { token });
      expect([403, 404], path).toContain(res.status);
    }
    const crossed = await call(`/workspaces/${seed.outsiderWorkspaceId}/messages/${seed.messages.timesheet}`, { token });
    expect(crossed.status).toBe(404);
    const crossedDraft = await call(`/workspaces/${seed.outsiderWorkspaceId}/messages/${seed.messages.timesheet}/draft`, {
      token,
      method: 'POST',
    });
    expect([403, 404]).toContain(crossedDraft.status);
  });
});

test.describe('user A opens user B’s workspace by URL', () => {
  test.use(signedInAs('admin'));

  test('shows "Workspace not found"', async ({ page, seed }) => {
    await page.goto(`/w/${seed.outsiderWorkspaceId}/inbox`);
    await expect(page.getByText('Workspace not found')).toBeVisible();
  });
});

test.describe('an operator of A', () => {
  test.use(signedInAs('operator'));

  test('cannot open B’s workspace either', async ({ page, seed }) => {
    await page.goto(`/w/${seed.outsiderWorkspaceId}/labels`);
    await expect(page.getByText('Workspace not found')).toBeVisible();
  });
});
