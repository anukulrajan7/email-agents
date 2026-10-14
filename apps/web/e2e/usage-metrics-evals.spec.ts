// Matrix: CO-02 (AI usage page: today vs budget, 30 days, cost per email, where it goes; admin only; no model names).
// MX-01, MX-02, MX-04, MX-05, MX-08 (metrics scorecard, agent overview, safety and replay runs, evals, AI health) are
// not on main yet.
import { expect, MODEL_NAMES, signedInAs, test } from './support/test';

test.describe('AI usage (CO-02)', () => {
  test.use(signedInAs('admin'));

  test('shows today against the daily budget, the last 30 days and the cost per email, with no model names', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/usage`);
    await expect(page.getByRole('heading', { name: 'AI usage' })).toBeVisible();
    await expect(page.getByText('Today', { exact: true })).toBeVisible();
    await expect(page.getByText(/of \$2\.00/)).toBeVisible();
    await expect(page.getByText('Last 30 days')).toBeVisible();
    await expect(page.getByText('Cost per email')).toBeVisible();
    await expect(page.getByText('Where it goes')).toBeVisible();
    expect(await page.locator('main').innerText()).not.toMatch(MODEL_NAMES);
  });

  test('with the AI mock nothing was spent: no AI work is listed', async ({ page, seed }) => {
    await page.goto(`/w/${seed.workspaceId}/usage`);
    await expect(page.getByText('No AI work yet')).toBeVisible();
    await expect(page.getByText('$0.00 of $2.00')).toBeVisible();
  });
});

test.fixme('MX-01 Metrics shows a scorecard (label accuracy, approval rate, edit distance, cost per email, safety) and an agent comparison', async () => {});
test.fixme('MX-02 an agent Overview shows approval rate, edit distance, p50/p95 latency and a 30-day series', async () => {});
test.fixme('MX-04 saving a new agent version shows safety and replay runs; a safety failure blocks anything above L0', async () => {});
test.fixme('MX-05 running an eval from the UI queues, finishes and shows a score against the baseline', async () => {});
test.fixme('MX-08 AI health lists incidents (leak, injection, disagreement, refusal, schema failure, hallucination, send blocked)', async () => {});
