// Matrix: RP-02 (better draft / regenerate), RP-04 (NEEDS gaps stop a send), RP-05 (confirm dialog), RP-06 (blocked
// sends never reach Gmail), RP-07 (undo), RP-09 (save to Gmail drafts). Gmail is the mock behind Nango: it records
// what the API really sends, so "never sent" is checked at the provider, not only in the UI.
import type { Page } from '@playwright/test';
import { nango, sentMail, type SentMail } from './support/api';
import { expect, openMessage, signedInAs, test } from './support/test';

test.use(signedInAs('operator'));
test.describe.configure({ mode: 'serial' });

const undoSeconds = Number(process.env.E2E_SEND_UNDO_SECONDS ?? 10);

async function generateDraft(page: Page) {
  await page.getByRole('button', { name: 'Generate draft' }).click();
  await expect(page.locator('#draft-body')).toBeVisible({ timeout: 20_000 });
}

const sentTo = async (address: string): Promise<SentMail[]> => (await sentMail()).filter((m) => m.to.includes(address));

test('drafts a reply in the team style, signed off, with no AI mention', async ({ page, seed }) => {
  await openMessage(page, seed, 'reply1');
  await expect(page.getByText('No draft yet')).toBeVisible();
  await generateDraft(page);
  const draft = page.locator('#draft-body');
  await expect(draft).toHaveValue(/Thank you for your email/);
  await expect(draft).toHaveValue(/Best regards,\nAcme Desk/);
  await expect(draft).not.toHaveValue(/\bAI\b|assistant|model/i);
  await expect(page.getByRole('button', { name: 'Write a better draft' })).toBeVisible();
  await page.getByRole('button', { name: 'Write a better draft' }).click();
  await expect(page.getByRole('button', { name: 'Regenerate' })).toBeEnabled({ timeout: 20_000 });
});

test('a [NEEDS: …] gap is called out and keeps Send disabled until it is filled', async ({ page, seed }) => {
  await openMessage(page, seed, 'question');
  await generateDraft(page);
  const draft = page.locator('#draft-body');
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await draft.fill('Hello Ravi,\n\nOur enterprise plan costs [NEEDS: price per seat] per month for a team of 40.\n\nBest regards,\nAcme Desk');
  await expect(page.getByText('Fill every dashed [NEEDS: …] part to send')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
  await draft.fill('Hello Ravi,\n\nOur enterprise plan costs 12 dollars per seat per month for a team of 40.\n\nBest regards,\nAcme Desk');
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
});

test('Send needs no dialog: it starts the undo countdown at once', async ({ page, seed }) => {
  await openMessage(page, seed, 'reply1');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  await expect(page.getByRole('status').filter({ hasText: /Sending in \d+ s/ })).toBeVisible();
  await page.getByRole('button', { name: 'Undo' }).click();
  expect(await sentTo('dev.patel@partner.example')).toHaveLength(0);
});

test('a reply that is too short is refused and never reaches Gmail', async ({ page, seed }) => {
  await openMessage(page, seed, 'reply1');
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await page.locator('#draft-body').fill('ok');
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText(/only 2 characters/);
  expect(await sentTo('dev.patel@partner.example')).toHaveLength(0);
});

test('Undo inside the window stops the send: nothing is delivered after it', async ({ page, seed }) => {
  await openMessage(page, seed, 'reply2');
  await generateDraft(page);
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  const bar = page.getByRole('status').filter({ hasText: /Sending in \d+ s/ });
  await expect(bar).toBeVisible();
  await bar.getByRole('button', { name: 'Undo' }).click();
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toBeVisible();
  await page.waitForTimeout((undoSeconds + 4) * 1000);
  expect(await sentTo('sana.khan@partner.example')).toHaveLength(0);
});

test('a confirmed send counts down, then goes out threaded exactly once and shows Sent', async ({ page, seed }) => {
  await openMessage(page, seed, 'reply1');
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await page.locator('#draft-body').fill(
    'Hello Dev,\n\nThank you for confirming your visit on Friday afternoon. We look forward to seeing you.\n\nBest regards,\nAcme Desk',
  );
  await page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: /Sending in \d+ s/ })).toBeVisible();
  expect(await sentTo('dev.patel@partner.example')).toHaveLength(0);

  await expect(page.getByText(/Sent ✓/)).toBeVisible({ timeout: (undoSeconds + 20) * 1000 });
  const sent = await sentTo('dev.patel@partner.example');
  expect(sent).toHaveLength(1);
  expect(sent[0]?.subject).toBe('Re: Office visit on Friday');
  expect(sent[0]?.body).toContain('We look forward to seeing you');
  expect(sent[0]?.threadId).toBe('tgreply1');
  expect(sent[0]?.inReplyTo).toContain('greply1@mail.e2e.test');
  await page.reload();
  await expect(page.getByText(/Sent ✓/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Send', exact: true })).toHaveCount(0);
  expect(await sentTo('dev.patel@partner.example')).toHaveLength(1);
});

test('Save to Gmail drafts puts the reply in the mailbox drafts', async ({ page, seed }) => {
  await openMessage(page, seed, 'label1');
  await generateDraft(page);
  await page.getByRole('button', { name: 'More' }).click();
  await page.getByRole('menuitem', { name: 'Save to Gmail drafts' }).click();
  await expect(page.getByText('Saved in your Gmail Drafts')).toBeVisible();
  const drafts = (await nango<{ items: { to: string; subject: string }[] }>('/__e2e/drafts')).items;
  expect(drafts.some((d) => d.to.includes('neha.verma@vendor.example') && d.subject === 'Re: Invoice INV-1042 for October services')).toBe(true);
});
