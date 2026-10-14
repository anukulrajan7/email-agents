import { defineConfig, devices } from '@playwright/test';
import { API_PORT, BASE_URL, NANGO_PORT } from './e2e/support/env';

const prod = Boolean(process.env.E2E_PROD);
const ci = Boolean(process.env.CI);

export default defineConfig({
  testDir: './e2e',
  testMatch: prod ? /smoke\.prod\.spec\.ts/ : /.*\.spec\.ts/,
  testIgnore: prod ? undefined : /smoke\.prod\.spec\.ts/,
  outputDir: './e2e/.results',
  fullyParallel: false,
  workers: 1,
  retries: ci ? 1 : 0,
  forbidOnly: ci,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: ci ? [['list'], ['html', { open: 'never', outputFolder: './e2e/.report' }]] : [['list']],
  globalSetup: prod ? undefined : './e2e/global-setup.ts',
  use: {
    baseURL: BASE_URL,
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
    viewport: { width: 1440, height: 900 },
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'], viewport: { width: 1440, height: 900 } } }],
  webServer: prod
    ? undefined
    : [
        {
          command: 'node e2e/support/mock-nango.mjs',
          url: `http://127.0.0.1:${NANGO_PORT}/__e2e/health`,
          reuseExistingServer: false,
          timeout: 30_000,
          stdout: 'pipe',
        },
        {
          command: 'node e2e/support/start-api.mjs',
          url: `http://127.0.0.1:${API_PORT}/health/ready`,
          reuseExistingServer: false,
          timeout: 600_000,
          stdout: 'pipe',
          stderr: 'pipe',
        },
      ],
});
