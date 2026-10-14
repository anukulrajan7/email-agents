// Matrix: IN-01 (list, sort, filter), IN-02 (reader header), IN-03 (sandboxed HTML), IN-05 (insight), IN-07 (mobile).
import { expect, openMessage, signedInAs, test } from './support/test';

test.use(signedInAs('operator'));

test.describe('inbox list', () => {
  test('lists every seeded email like a mail client, with label, reply status and attachment count', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    const list = page.getByRole('list', { name: 'Messages in this view' });
    await expect(list.getByRole('listitem')).toHaveCount(9);
    const timesheet = list.getByRole('link', { name: /September timesheet/ });
    await expect(timesheet).toContainText('priya.sharma');
    await expect(timesheet).toContainText('1 attachment');
    await expect(timesheet).toContainText(/Needs reply/i);
    await expect(list.getByRole('link', { name: /Invoice INV-1042/ })).toContainText(/billing/i);
  });

  test('puts the urgent mail first by importance and remembers the Newest choice after a reload', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    const links = page.getByRole('list', { name: 'Messages in this view' }).getByRole('link');
    await expect(links.first()).toContainText('URGENT: production outage');
    await page.getByRole('button', { name: 'Newest' }).click();
    await expect(links.first()).toContainText('Java Developer requirement');
    await page.reload();
    await expect(page.getByRole('button', { name: 'Newest' })).toHaveAttribute('aria-pressed', 'true');
    await expect(links.first()).toContainText('Java Developer requirement');
    await page.getByRole('button', { name: 'Important first' }).click();
    await expect(links.first()).toContainText('URGENT: production outage');
  });

  test('filters by label and by reply status, and searches subject and body', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    const items = page.getByRole('list', { name: 'Messages in this view' }).getByRole('listitem');
    await page.getByLabel('Filter by label').selectOption({ label: 'Billing' });
    await expect(items).toHaveCount(2);
    await page.getByLabel('Filter by label').selectOption({ label: 'Replied' });
    await expect(page.getByText('No messages match')).toBeVisible();
    await page.getByLabel('Filter by label').selectOption({ label: 'All labels' });
    await expect(items).toHaveCount(9);
    await page.getByLabel('Search messages').fill('pricing');
    await expect(items).toHaveCount(1);
    await expect(items.first()).toContainText('Question about pricing');
  });

  test('summaries never show internal attachment markup', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    const row = page.getByRole('link', { name: /September timesheet/ });
    await expect(row).toBeVisible();
    await expect(row).not.toContainText('<attachment', { timeout: 2_000 });
  });

  test('finds an email by a word that only appears in its body', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await page.getByLabel('Search messages').fill('enterprise plan');
    const items = page.getByRole('list', { name: 'Messages in this view' }).getByRole('listitem');
    await expect(items).toHaveCount(1, { timeout: 3_000 });
  });
});

test.describe('reader', () => {
  test('shows a Gmail-style header with sender, recipients and a details popover', async ({ page, seed }) => {
    await openMessage(page, seed, 'timesheet');
    const article = page.getByRole('article', { name: 'Email' });
    await expect(article.getByRole('heading', { name: 'September timesheet' })).toBeVisible();
    await expect(article).toContainText('Priya Sharma');
    await expect(article).toContainText('<priya.sharma@clientco.example>');
    await expect(article).toContainText('to desk');
    await article.getByRole('button', { name: 'Show details' }).click();
    const details = article.getByRole('group', { name: 'Message details' });
    await expect(details).toContainText('desk@acme-desk.example');
    await expect(details).toContainText('priya.sharma@clientco.example');
    await page.keyboard.press('Escape');
    await expect(details).toBeHidden();
  });

  test('has no pipeline tracker in the reader and no raw job names', async ({ page, seed }) => {
    await openMessage(page, seed, 'timesheet');
    const text = await page.locator('main').innerText();
    expect(text).not.toMatch(/pipeline|job_id|processing_status|normalized/i);
  });

  test('summarises the mail in an insight card with the ask and reply status', async ({ page, seed }) => {
    await openMessage(page, seed, 'question');
    await expect(page.getByText("WHAT THEY'RE ASKING", { exact: false })).toBeVisible();
    await expect(page.getByRole('article', { name: 'Email' }).getByText(/needs reply/i).first()).toBeVisible();
  });

  test('renders HTML mail in a sandboxed frame where scripts and remote styles never run', async ({ page, seed }) => {
    await openMessage(page, seed, 'newsletter');
    const frame = page.locator('iframe[title="Email content"]');
    await expect(frame).toBeVisible();
    const sandbox = (await frame.getAttribute('sandbox')) ?? '';
    expect(sandbox).not.toContain('allow-scripts');
    await expect(page.frameLocator('iframe[title="Email content"]').getByRole('heading', { name: 'Digest title' })).toBeVisible();
    const pwned = await page.evaluate(() => (window as unknown as { __pwned?: boolean }).__pwned);
    expect(pwned).toBeUndefined();
    await expect(page).not.toHaveTitle(/pwned/);
    const html = await frame.getAttribute('srcdoc');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('evil.example');
    await page.getByRole('button', { name: 'More message actions' }).click();
    await page.getByRole('menuitem', { name: 'View plain text' }).click();
    await expect(page.getByRole('region', { name: 'Email body' })).toContainText('five articles about databases');
    await expect(frame).toHaveCount(0);
  });

  test('stacks list and detail on a phone and has a Back control', async ({ page, seed }) => {
    await page.setViewportSize({ width: 390, height: 800 });
    await page.goto(`/w/${seed.workspaceId}/inbox`);
    await expect(page.getByRole('region', { name: 'Messages' })).toBeVisible();
    await page.getByRole('link', { name: /Question about pricing/ }).click();
    await expect(page.getByRole('article', { name: 'Email' })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Messages' })).toBeHidden();
    await page.getByRole('link', { name: /Back to inbox/ }).click();
    await expect(page.getByRole('region', { name: 'Messages' })).toBeVisible();
  });
});
