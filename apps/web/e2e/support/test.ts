import { expect, test as base, type Page } from '@playwright/test';
import { seed as readSeed, type Seed } from './api';
import { authFile, type Role } from './env';

export { expect };

/** `test` with the seeded workspace, users and message ids from global setup. */
export const test = base.extend<{ seed: Seed }>({
  seed: async ({}, use) => {
    await use(readSeed());
  },
});

export const signedInAs = (role: Role) => ({ storageState: authFile(role) });
export const signedOut = { storageState: { cookies: [], origins: [] } };

/** Words that would reveal which AI model or provider is behind the product. */
export const MODEL_NAMES =
  /\b(deepseek|qwen|kimi|moonshot|openrouter|typesafe|jev-latest|gpt-?\d|claude|gemini|llama|mistral|baai|bge-m3|template-v1|rules-v1|scripted)/i;

export async function openMessage(page: Page, seed: Seed, key: string) {
  await page.goto(`/w/${seed.workspaceId}/inbox/${seed.messages[key]}`);
  await expect(page.getByRole('article', { name: 'Email' })).toBeVisible();
}

export const sidebar = (page: Page) => page.getByRole('navigation', { name: 'Main' });
