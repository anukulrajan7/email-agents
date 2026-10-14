// AG-01..05, AG-08, PK-05 (branch: agents-hub, agent registry)
// Stub: the feature is not on main yet, or needs seed data this suite does not have.
// Row ids refer to docs/acceptance-matrix.md.
import { test } from './support/test';

test.fixme("AG-01 an admin sees the Agents hub grouped by category for the enabled packs, with avatar, status toggle, autonomy chip and today's counts", async () => {});
test.fixme('AG-02 enabling a pack twice seeds once; disabling pauses its agents (pack_off) and deletes nothing; re-enabling restores them', async () => {});
test.fixme('AG-03 deleting the lead or a system agent is refused and the delete button is hidden; pause and tune still work', async () => {});
test.fixme('AG-04 tuning a system agent marks it tuned; a new template version shows Update available with a diff', async () => {});
test.fixme('AG-05 cloning a system agent creates a custom agent with cloned_from and a copy of the spec', async () => {});
test.fixme('AG-08 Pause all pauses every agent, nothing runs, and resume restores them', async () => {});
test.fixme('PK-05 switching Support to Sales pauses support agents (pack_off), seeds Sales agents at L0, adds labels and keeps cases and history', async () => {});
