// Matrix: AT-01 (PDF), AT-03 (XLSX preview table), AT-06 (image, stubbed vision reader). The mail comes in through the
// Gmail sync path (mock Nango), so these files were downloaded and read by the real attachment jobs.
import { call } from './support/api';
import { expect, openMessage, signedInAs, test } from './support/test';

test.use(signedInAs('operator'));

const chip = (page: import('@playwright/test').Page, name: string) =>
  page.getByRole('region', { name: 'Attachments' }).getByRole('listitem').filter({ hasText: name });

test('a spreadsheet is read and previewed as a table', async ({ page, seed }) => {
  await openMessage(page, seed, 'timesheet');
  const sheet = chip(page, 'timesheet-september.xlsx');
  await expect(sheet).toContainText('XLSX');
  await expect(sheet).toContainText('Read by AI');
  await sheet.getByRole('button', { name: 'Show preview' }).click();
  const table = sheet.getByRole('table', { name: 'Timesheet sample' });
  await expect(table).toContainText('Employee');
  await expect(table).toContainText('Falconwing');
  await expect(sheet).toContainText('4 rows');
});

test('a PDF is read and its words show in the preview', async ({ page, seed }) => {
  await openMessage(page, seed, 'java');
  const pdf = chip(page, 'job-description.pdf');
  await expect(pdf).toContainText('PDF');
  await pdf.getByRole('button', { name: 'Show preview' }).click();
  await expect(pdf).toContainText('Java Developer requirement');
  await expect(pdf).toContainText('Capgemini');
});

test('an image gets a description, its text and the fields found in it', async ({ page, seed }) => {
  await openMessage(page, seed, 'receipt');
  const image = chip(page, 'receipt.png');
  await image.getByRole('button', { name: 'Show preview' }).click();
  await expect(image).toContainText('Receipt');
  await expect(image).toContainText('A cafe receipt');
  await expect(image).toContainText('Total 42 Mumbai Cafe');
});

test('a word that exists only inside an attachment finds the email in search', async ({ seed }) => {
  const res = await call<{ items: { message_id: string; kind: string }[] }>(
    `/workspaces/${seed.workspaceId}/search?q=Quillfeather&limit=5`,
    { token: seed.operator.token },
  );
  expect(res.status).toBe(200);
  expect(res.body.items[0]?.message_id).toBe(seed.messages.java);
  expect(res.body.items[0]?.kind).toBe('attachment');
  const sheet = await call<{ items: { message_id: string }[] }>(`/workspaces/${seed.workspaceId}/search?q=Falconwing&limit=5`, {
    token: seed.operator.token,
  });
  expect(sheet.body.items[0]?.message_id).toBe(seed.messages.timesheet);
});

// BUG found by this suite: no attachment scanner exists, so every file says "Scan pending" forever
// (`attachments.scan_status` is never moved off its default). Remove `test.fail` once the chip stops saying so.
test.fail('a read attachment does not claim a scan is still pending', async ({ page, seed }) => {
  await openMessage(page, seed, 'timesheet');
  await expect(chip(page, 'timesheet-september.xlsx')).not.toContainText('Scan pending', { timeout: 2_000 });
});

test('attachment chips are listed in the message list with their count', async ({ page, seed }) => {
  await page.goto(`/w/${seed.workspaceId}/inbox`);
  for (const subject of ['September timesheet', 'Java Developer requirement', 'Cafe receipt']) {
    await expect(page.getByRole('link', { name: new RegExp(subject) })).toContainText('1 attachment');
  }
});
