// Matrix: AM-01 (Ctrl+K, cited answer; the planner is the AI_MOCK scripted one: it searches for the request's
// likeliest keyword, then answers from the first hit). The planner never sees mail bodies; that is an API test.
import { expect, signedInAs, test } from './support/test';

test.use(signedInAs('operator'));

test('Ctrl+K opens Ask Meera, a typed question gets a cited answer that opens the email, Esc closes it', async ({ page, seed }) => {
  await page.goto(`/w/${seed.workspaceId}/inbox`);
  await expect(page.getByRole('dialog', { name: 'Ask Meera' })).toHaveCount(0);
  await expect(page.getByRole('button', { name: 'Ask Meera' })).toBeVisible();
  await page.keyboard.press('Control+k');
  const panel = page.getByRole('dialog', { name: 'Ask Meera' });
  await expect(panel).toBeVisible();
  await expect(page.getByLabel('Message to Meera')).toBeFocused();

  await page.getByLabel('Message to Meera').fill('What did Capgemini send about Java?');
  await panel.getByRole('button', { name: 'Send' }).click();

  const conversation = panel.getByRole('list', { name: 'Conversation' });
  await expect(conversation).toContainText('You said: What did Capgemini send about Java?');
  await expect(conversation).toContainText('Meera said:', { timeout: 20_000 });
  await expect(conversation).toContainText('Java Developer requirement at Capgemini');
  const source = conversation.getByRole('link', { name: 'Java Developer requirement at Capgemini' });
  await expect(source).toBeVisible();
  await expect(conversation.getByText('Sources')).toBeVisible();
  await expect(conversation).not.toContainText('^');

  await source.click();
  await expect(page).toHaveURL(new RegExp(`/inbox/${seed.messages.java}$`));
  await expect(page.getByRole('article', { name: 'Email' })).toContainText('Java Developer position');

  await page.keyboard.press('Escape');
  await expect(panel).toBeHidden();
});

test('a word that only appears inside an attachment is found too', async ({ page, seed }) => {
  await page.goto(`/w/${seed.workspaceId}/inbox`);
  await page.getByRole('button', { name: 'Ask Meera' }).click();
  await page.getByLabel('Message to Meera').fill('Quillfeather');
  const panel = page.getByRole('dialog', { name: 'Ask Meera' });
  await panel.getByRole('button', { name: 'Send' }).click();
  await expect(panel.getByRole('link', { name: 'Java Developer requirement at Capgemini' }).first()).toBeVisible({ timeout: 20_000 });
});

test('the example prompts are clickable', async ({ page, seed }) => {
  await page.route('**/assistant/history*', (route) => route.fulfill({ json: { items: [], next_before: null } }));
  await page.goto(`/w/${seed.workspaceId}/inbox`);
  await page.getByRole('button', { name: 'Ask Meera' }).click();
  const panel = page.getByRole('dialog', { name: 'Ask Meera' });
  await expect(panel.getByRole('button', { name: 'What needs a reply?' })).toBeVisible();
  await panel.getByRole('button', { name: 'What needs a reply?' }).click();
  await expect(panel.getByRole('list', { name: 'Conversation' })).toContainText('Meera said:', { timeout: 20_000 });
});
