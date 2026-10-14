// Matrix: SU-01 (sign up, enter the emailed code), SU-02 (closed sign-up), SU-04 (UI message for wrong passwords),
// SU-05 (forgot password). The verification email is captured by the API (MAIL_CAPTURE_FILE) instead of sent.
import { mailTo, otpFor } from './support/api';
import { PASSWORD, SIGNUP_DOMAIN } from './support/env';
import { expect, signedOut, test } from './support/test';

test.use(signedOut);
test.describe.configure({ mode: 'serial' });

const email = `sam.${Date.now() % 1_000_000}@${SIGNUP_DOMAIN}`;

test('signs up, is asked for the emailed code, rejects a wrong one, and lands on the workspace picker', async ({ page }) => {
  await page.goto('/sign-up');
  await page.getByLabel('Name').fill('Sam Signup');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password', { exact: true }).fill('short');
  await expect(page.getByRole('button', { name: 'Create account' })).toBeDisabled();
  await expect(page.getByText('5/10 — use at least 10 characters')).toBeVisible();
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Create account' }).click();

  await expect(page.getByRole('heading', { name: 'Check your email' })).toBeVisible();
  await expect(page.getByText(email)).toBeVisible();
  await expect(page.getByText('Step 2 of 2 · Verify')).toBeVisible();
  await expect(page.getByRole('button', { name: 'Verify' })).toBeDisabled();

  const code = await otpFor(email);
  const wrong = code === '000000' ? '111111' : '000000';
  await page.getByLabel('Verification code').fill(wrong);
  await page.getByRole('button', { name: 'Verify' }).click();
  await expect(page.getByText('That code is invalid or expired.')).toBeVisible();

  await page.getByLabel('Verification code').fill(code);
  await page.getByRole('button', { name: 'Verify' }).click();
  await expect(page).toHaveURL(/\/workspaces$/);
  await expect(page.getByRole('heading', { name: 'Set up your first workspace' })).toBeVisible();
  await expect(page.getByText(email)).toBeVisible();
});

test('signing out returns to the sign-in page and a wrong password gets a plain message', async ({ page }) => {
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password', { exact: true }).fill('not-the-password');
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByText('Email or password is incorrect')).toBeVisible();

  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/workspaces$/);
  await page.getByRole('button', { name: 'Sign out' }).click();
  await expect(page).toHaveURL(/\/sign-in$/);
  await page.goto('/workspaces');
  await expect(page).toHaveURL(/\/sign-in$/);
});

test('an address that already has an account is told so, with a way to sign in', async ({ page }) => {
  await page.goto('/sign-up');
  await page.getByLabel('Email').fill(email);
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByText('An account with this email already exists')).toBeVisible();
  await expect(page.getByRole('link', { name: 'Go to sign in' })).toBeVisible();
});

test('sign-up is closed to addresses that are not on the allow-list', async ({ page }) => {
  await page.goto('/sign-up');
  await page.getByLabel('Email').fill('stranger@not-allowed.example');
  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Create account' }).click();
  await expect(page.getByText('Sign-up is by invitation. Ask a workspace admin to invite you.')).toBeVisible();
});

test('forgot password: a code by email sets a new password and the old one stops working', async ({ page }) => {
  const NEW_PASSWORD = 'a-brand-new-password-42';
  await page.goto('/sign-in');
  await page.getByLabel('Email').fill(email);
  await page.getByRole('button', { name: 'Forgot password?' }).click();
  await expect(page.getByRole('heading', { name: 'Reset your password' })).toBeVisible();
  const before = mailTo(email).length;
  await page.getByRole('button', { name: 'Send code' }).click();
  await expect(page.getByRole('heading', { name: 'Choose a new password' })).toBeVisible();

  const code = await otpFor(email, before);
  await page.getByLabel('Code').fill(code);
  await page.getByLabel('New password').fill(NEW_PASSWORD);
  await page.getByRole('button', { name: 'Set new password' }).click();
  await expect(page.getByText('Password changed. Sign in with your new password.')).toBeVisible();

  await page.getByLabel('Password', { exact: true }).fill(PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByText('Email or password is incorrect')).toBeVisible();
  await page.getByLabel('Password', { exact: true }).fill(NEW_PASSWORD);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page).toHaveURL(/\/workspaces$/);
});
