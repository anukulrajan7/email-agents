// AP-01..03, AG-12 (needs the agent planner, which AI_MOCK does not script yet; vitest covers the components)
// Stub: the feature is not on main yet, or needs seed data this suite does not have.
// Row ids refer to docs/acceptance-matrix.md.
import { test } from './support/test';

test.fixme('AP-01 Waiting for you lists recipient, subject and scores with a nav badge count', async () => {});
test.fixme('AP-02 approving edited and unedited, declining with a note, and a stale approval returning 409 inline', async () => {});
test.fixme('AP-03 approving, then editing the content voids the approval by content hash; it must be approved again', async () => {});
test.fixme('AG-12 opening a case shows a timeline with summaries, cost and budget, inline approval, and admin raw JSON and cancel', async () => {});
