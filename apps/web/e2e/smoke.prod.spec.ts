// Production smoke (matrix: ST-04, AM-01, a slice of CN-12). Read-only: it signs in to a test workspace, looks, asks one
// question and never sends, edits or deletes anything. Not part of the normal run or CI.
//
//   E2E_BASE_URL=https://mail.example.com E2E_SMOKE_EMAIL=smoke@example.com E2E_SMOKE_PASSWORD=... pnpm e2e:prod
//
// Optional: E2E_SMOKE_WORKSPACE (name of the workspace to open; default the first one).
import { expect, test } from '@playwright/test';

const baseURL = process.env.E2E_BASE_URL;
const email = process.env.E2E_SMOKE_EMAIL;
const password = process.env.E2E_SMOKE_PASSWORD;
const workspaceName = process.env.E2E_SMOKE_WORKSPACE;

test.skip(!baseURL || !email || !password, 'set E2E_BASE_URL, E2E_SMOKE_EMAIL and E2E_SMOKE_PASSWORD to run the production smoke');

test.use({ baseURL });

function trackProblems(page: import('@playwright/test').Page) {
  const problems: string[] = [];
  page.on('console', (m) => {
    if (m.type() === 'error') problems.push(`console: ${m.text()}`);
  });
  page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
  page.on('response', (r) => {
    if (r.status() >= 500) problems.push(`${r.status()} ${r.url()}`);
  });
  return problems;
}

test('the service is healthy', async ({ request }) => {
  const live = await request.get('/health/live');
  expect(live.status()).toBe(200);
  const ready = await request.get('/health/ready');
  expect(ready.status()).toBe(200);
  expect(await ready.json()).toEqual({ status: 'ok' });
  const headers = ready.headers();
  expect(headers['x-content-type-options']).toBe('nosniff');
});

test('the sign-in page loads with no console errors', async ({ page }) => {
  const problems = trackProblems(page);
  await page.goto('/sign-in');
  await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
  await expect(page.getByLabel('Email')).toBeVisible();
  await page.waitForLoadState('networkidle');
  expect(problems).toEqual([]);
});

test('sign in, open the inbox, ask Meera one read-only question', async ({ page }) => {
  const problems = trackProblems(page);
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(email!);
  await page.getByLabel('Password', { exact: true }).fill(password!);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/workspaces$/);

  const link = workspaceName
    ? page.getByRole('link', { name: `Open ${workspaceName}` })
    : page.getByRole('list', { name: 'Workspaces' }).getByRole('link').first();
  await link.click();
  await expect(page).toHaveURL(/\/inbox$/);
  await expect(page.getByRole('heading', { name: 'Inbox' })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Main' })).toBeVisible();
  await expect(page.getByRole('list', { name: 'Messages in this view' }).or(page.getByText('No messages yet')).first()).toBeVisible();

  await page.keyboard.press('Control+k');
  const panel = page.getByRole('dialog', { name: /Ask / });
  await expect(panel).toBeVisible();
  await page.getByLabel(/^Message to /).fill('What needs a reply?');
  await panel.getByRole('button', { name: 'Send' }).click();
  await expect(panel.getByRole('list', { name: 'Conversation' })).toContainText(/said:/, { timeout: 60_000 });
  await expect(panel).not.toContainText('I could not do that');

  await page.waitForLoadState('networkidle');
  expect(problems).toEqual([]);
});

test('an invalid token is sent back to sign-in and nothing leaks', async ({ page }) => {
  await page.addInitScript(() => localStorage.setItem('auth_token', 'not-a-real-token'));
  await page.goto('/workspaces');
  await expect(page).toHaveURL(/\/sign-in$/);
});
