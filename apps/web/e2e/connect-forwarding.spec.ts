// Matrix: CN-06 (forwarding address, test email lands), CN-08 (disconnect a mailbox). CN-01 (the real Gmail consent
// popup) cannot run in a browser test; the mailbox is registered through the same API the popup calls.
import { call, createWorkspace, nango, ok, seed } from './support/api';
import { expect, signedInAs, test } from './support/test';

test.use(signedInAs('admin'));
test.describe.configure({ mode: 'serial' });

let lab = '';

test.beforeAll(async () => {
  lab = await createWorkspace(seed().admin, `Connect Lab ${Date.now() % 100000}`);
});

test('an admin creates the forwarding address once and a test email lands in the inbox', async ({ page }) => {
  await page.goto(`/w/${lab}/integrations`);
  await expect(page.getByRole('heading', { name: 'Forward emails' })).toBeVisible();
  await page.getByRole('button', { name: 'Create forwarding address' }).click();
  const address = page.locator('code').filter({ hasText: '@inbound.e2e.test' });
  await expect(address).toBeVisible();
  const first = await address.innerText();

  await page.reload();
  await expect(page.locator('code').filter({ hasText: '@inbound.e2e.test' })).toHaveText(first);
  expect(await page.getByRole('button', { name: 'Create forwarding address' }).count()).toBe(0);

  await page.getByRole('button', { name: 'Send test email' }).click();
  await expect(page.getByRole('status').filter({ hasText: /queued|on its way|few seconds/i })).toBeVisible();

  await page.goto(`/w/${lab}/inbox`);
  const row = page.getByRole('link', { name: /September timesheet/ });
  await expect(row).toBeVisible({ timeout: 30_000 });
  await expect(row).toContainText('priya.sharma');
  await expect(row).toContainText('1 attachment');
  await row.click();
  await expect(page.getByRole('article', { name: 'Email' })).toContainText('168 hours');
  await expect(page.getByText("Forwarded emails can't create drafts in the original mailbox")).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toHaveCount(0);
});

test('creating the forwarding source again returns the same one (idempotent)', async () => {
  const token = seed().admin.token;
  const a = await ok<{ id: string; inbound_address: string }>(`/workspaces/${lab}/sources/forwarding`, { token, method: 'POST' });
  const b = await ok<{ id: string; inbound_address: string }>(`/workspaces/${lab}/sources/forwarding`, { token, method: 'POST' });
  expect(b.id).toBe(a.id);
  expect(b.inbound_address).toBe(a.inbound_address);
});

test('a connected Gmail mailbox can be disconnected, after a confirmation, and stops syncing', async ({ page }) => {
  const s = seed();
  await nango('/__e2e/connections', { connection_id: 'conn_connect_lab', organization_id: lab, end_user_id: s.admin.id });
  await nango('/__e2e/mailbox', { connection_id: 'conn_connect_lab', address: 'lab@acme-desk.example', messages: [], attachments: {} });
  const source = await ok<{ id: string; status: string }>(`/workspaces/${lab}/sources/nango-connections`, {
    token: s.admin.token,
    body: { connection_id: 'conn_connect_lab', provider_config_key: 'google-mail' },
  });
  expect(source.status).toBe('connected');
  await call(`/workspaces/${lab}/me/agent-profile/skip`, { token: s.admin.token, method: 'POST' });

  await page.goto(`/w/${lab}/integrations`);
  await expect(page.getByRole('button', { name: 'Sync now' })).toBeVisible();
  await expect(page.getByText('CAN SAVE DRAFTS', { exact: false }).first()).toBeVisible();

  await page.getByRole('button', { name: 'Disconnect' }).click();
  const dialog = page.getByRole('dialog', { name: 'Disconnect Gmail?' });
  await expect(dialog).toContainText('Emails already here stay in Post Room.');
  await dialog.getByRole('button', { name: 'Cancel' }).click();
  await expect(dialog).toBeHidden();
  await expect(page.getByRole('button', { name: 'Sync now' })).toBeVisible();

  await page.getByRole('button', { name: 'Disconnect' }).click();
  await page.getByRole('dialog', { name: 'Disconnect Gmail?' }).getByRole('button', { name: 'Disconnect' }).click();
  await expect(page.getByRole('button', { name: 'Connect Gmail' })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Sync now' })).toHaveCount(0);

  const sync = await call(`/workspaces/${lab}/sources/${source.id}/sync`, { token: s.admin.token, method: 'POST' });
  expect(sync.status).toBe(409);
});
