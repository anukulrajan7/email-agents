// Matrix: SU-01, SU-07, SU-08, SU-14 (nav), PK-04. A new person signs up in the browser, reads the code from the captured
// mail, and creates a workspace with a pack; the sidebar and labels must match that pack. The goal-first onboarding
// wizard with agent rosters (SU-09..SU-16) is not built yet: those rows are fixme at the bottom.
import type { Page } from '@playwright/test';
import { otpFor, signUp, type Session } from './support/api';
import { PASSWORD, SIGNUP_DOMAIN } from './support/env';
import { expect, sidebar, signedOut, test } from './support/test';

test.use(signedOut);
test.describe.configure({ mode: 'serial' });

const stamp = Date.now() % 1_000_000;
const HR_LABELS = ['Timesheet', 'Candidate referral', 'Client requirement', 'Payroll'];

async function signInWithToken(page: Page, user: Session) {
  await page.addInitScript((token) => localStorage.setItem('auth_token', token), user.token);
}

async function createWorkspace(page: Page, name: string, packs: string[]) {
  await page.goto('/workspaces');
  await page.getByLabel('Workspace name').fill(name);
  const business = page.getByRole('checkbox', { name: /^Business inbox/ });
  if (await business.isChecked()) await business.uncheck();
  for (const pack of packs) await page.getByRole('checkbox', { name: new RegExp(`^${pack}`) }).check();
  await page.getByRole('button', { name: 'Create' }).click();
  await expect(page).toHaveURL(/\/w\/[^/]+\/inbox$/);
}

async function labelNames(page: Page): Promise<string> {
  await sidebar(page).getByRole('link', { name: 'Labels' }).click();
  const list = page.getByRole('list', { name: 'Labels' });
  await expect(list).toBeVisible();
  return (await list.textContent()) ?? '';
}

test('a new person signs up, enters the emailed code and creates a General workspace', async ({ page }) => {
  const email = `nina.${stamp}@${SIGNUP_DOMAIN}`;
  await page.goto('/sign-up');
  await page.getByLabel('Name').fill('Nina Newcomer');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Create account' }).click();
  await page.getByLabel('Verification code').fill(await otpFor(email));
  await page.getByRole('button', { name: 'Verify' }).click();

  await expect(page.getByRole('heading', { name: 'Set up your first workspace' })).toBeVisible();
  const create = page.getByRole('button', { name: 'Create' });
  await expect(create).toBeDisabled();
  await page.getByLabel('Workspace name').fill('Nina General');
  await expect(page.getByRole('checkbox', { name: /^Business inbox/ })).toBeChecked();
  for (const pack of ['HR & recruiting', 'Customer support', 'Sales', 'Finance', 'Personal']) {
    await expect(page.getByRole('checkbox', { name: new RegExp(`^${pack}`) })).not.toBeChecked();
  }
  await create.click();

  await expect(page).toHaveURL(/\/w\/[^/]+\/inbox$/);
  const nav = sidebar(page);
  await expect(page.getByText('Nina General').first()).toBeVisible();
  for (const name of ['Inbox', 'Agents', 'Alerts', 'Labels', 'Memory', 'Automations', 'Integrations', 'Packs']) {
    await expect(nav.getByRole('link', { name })).toBeVisible();
  }
  await expect(nav.getByRole('link', { name: 'Matching' })).toHaveCount(0);
  await expect(nav.getByRole('link', { name: 'Workflows' })).toHaveCount(0);
  await expect(page.getByText('No messages yet')).toBeVisible();

  const labels = await labelNames(page);
  expect(labels).toContain('Action required');
  expect(labels).toContain('Billing');
  for (const hr of HR_LABELS) expect(labels).not.toContain(hr);
});

test.describe('packs chosen at creation', () => {
  let user: Session;

  test.beforeAll(async () => {
    user = await signUp(`pat.${stamp}@${SIGNUP_DOMAIN}`, 'Pat Packs');
  });

  test('HR & recruiting adds Matching, Workflows and the HR labels', async ({ page }) => {
    await signInWithToken(page, user);
    await createWorkspace(page, 'Pat Staffing', ['HR & recruiting']);
    const nav = sidebar(page);
    await expect(nav.getByRole('link', { name: 'Matching' })).toBeVisible();
    await expect(nav.getByRole('link', { name: 'Workflows' })).toBeVisible();
    const labels = await labelNames(page);
    for (const hr of HR_LABELS) expect(labels).toContain(hr);
    expect(labels).not.toContain('Bug report');
  });

  const others: [string, string, string[]][] = [
    ['Customer support', 'Support', ['Bug report', 'Complaint', 'Refund request']],
    ['Sales', 'Sales', ['Sales lead', 'Follow-up', 'Scheduling']],
    ['Finance', 'Finance', ['Invoice', 'Payment', 'Expense']],
    ['Personal', 'Personal', ['Important', 'Needs reply', 'Newsletters', 'Receipts']],
  ];

  for (const [pack, title, expected] of others) {
    test(`${pack} brings its own labels and no HR pages`, async ({ page }) => {
      await signInWithToken(page, user);
      await createWorkspace(page, `Pat ${title}`, [pack]);
      const nav = sidebar(page);
      await expect(nav.getByRole('link', { name: 'Matching' })).toHaveCount(0);
      await expect(nav.getByRole('link', { name: 'Workflows' })).toHaveCount(0);
      const labels = await labelNames(page);
      for (const label of expected) expect(labels, `${pack} labels`).toContain(label);
      for (const hr of HR_LABELS) expect(labels).not.toContain(hr);
    });
  }

  test('two packs together bring both sets of labels', async ({ page }) => {
    const second = await signUp(`pia.${stamp}@${SIGNUP_DOMAIN}`, 'Pia Packs');
    await signInWithToken(page, second);
    await createWorkspace(page, 'Pat Support and Sales', ['Customer support', 'Sales']);
    const labels = await labelNames(page);
    expect(labels).toContain('Refund request');
    expect(labels).toContain('Sales lead');
  });

  test('a workspace needs a name and at least one pack before Create is enabled', async ({ page }) => {
    await signInWithToken(page, user);
    await page.goto('/workspaces');
    const create = page.getByRole('button', { name: 'Create' });
    await page.getByLabel('Workspace name').fill('No pack');
    await page.getByRole('checkbox', { name: /^Business inbox/ }).uncheck();
    await expect(create).toBeDisabled();
    await page.getByRole('checkbox', { name: /^Finance/ }).check();
    await expect(create).toBeEnabled();
    await page.getByLabel('Workspace name').fill('');
    await expect(create).toBeDisabled();
  });

  test('the picker lists every pack with a plain description and its labels', async ({ page }) => {
    await signInWithToken(page, user);
    await page.goto('/workspaces');
    const picker = page.getByRole('group', { name: 'What is this workspace for?' });
    for (const [name, text] of [
      ['Business inbox', 'Things to act on, questions, customers, leads, billing and scheduling.'],
      ['HR & recruiting', 'Timesheets, leave, referrals, client requirements, payroll and candidate matching.'],
      ['Customer support', 'Customer questions, bug reports, complaints and refund requests.'],
      ['Sales', 'New leads, follow-ups on proposals and meeting scheduling.'],
      ['Finance', 'Vendor invoices, payments and collections, expenses and reimbursements.'],
      ['Personal', "One person's inbox: what matters, what needs a reply, and the noise."],
    ] as const) {
      await expect(picker.getByRole('checkbox', { name: new RegExp(`^${name}`) })).toBeVisible();
      await expect(picker).toContainText(text);
    }
  });
});

// Goal-first onboarding with agent rosters, a 5-question profile and a dry-run (docs/acceptance-matrix.md §1) is not built.
test.fixme('SU-09 General: goal → packs → roster shows Lead and Inbox Assistant at L1 → connect → profile → first results', async () => {});
test.fixme('SU-10 Support roster adds Ticket Resolver and Escalations (both L0, active)', async () => {});
test.fixme('SU-11 Sales roster adds Lead Qualifier and Follow-ups (L0)', async () => {});
test.fixme('SU-12 Finance roster adds Invoices & Payments (L0, never promises payment)', async () => {});
test.fixme('SU-13 Personal roster adds Scheduling (L0, [NEEDS: slot])', async () => {});
test.fixme('SU-14 Staffing roster adds five HRMS agents; Matching and Workflows appear in the nav', async () => {});
test.fixme('SU-15 a fresh workspace has an active roster in under three minutes and no model name anywhere', async () => {});
test.fixme('SU-16 first visit to each page shows a tour, a "What is this?" help and plain-English empty states', async () => {});
