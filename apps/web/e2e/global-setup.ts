import { mkdirSync, writeFileSync } from 'node:fs';
import { call, nango, ok, signUp, type Seed, type Session } from './support/api';
import { AUTH_DIR, authFile, BASE_URL, type Role, SEED_FILE, SIGNUP_DOMAIN } from './support/env';
import { GUIDES } from '../src/features/help/guides';
import { gmailPayload, MAILBOX, MAILS } from './support/gmail';

const GUIDE_IDS = GUIDES.map((g) => g.id);

/** Signed-in browser state. The first-run checklist, and the per-page explanation cards are dismissed so they never cover the page under test. */
function saveSession(role: Role, session: Session, workspaceIds: string[]) {
  mkdirSync(AUTH_DIR, { recursive: true });
  const localStorage = [
    { name: 'auth_token', value: session.token },
    ...workspaceIds.map((id) => ({ name: `getting-started:${id}:dismissed`, value: '1' })),
    ...workspaceIds.map((id) => ({ name: `guide:v2:${session.id}:${id}:dismissed`, value: JSON.stringify(GUIDE_IDS) })),
  ];
  writeFileSync(authFile(role), JSON.stringify({ cookies: [], origins: [{ origin: BASE_URL, localStorage }] }));
}

type Page = { items: { id: string; subject: string; processing_status: string; attachment_count: number }[] };
type Detail = { attachments: { name: string; extract_status?: string | null }[] };

async function waitFor<T>(what: string, probe: () => Promise<T | undefined>, ms = 90_000): Promise<T> {
  const deadline = Date.now() + ms;
  for (;;) {
    const value = await probe();
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 500));
  }
}

export default async function globalSetup() {
  const admin = await signUp(`alice@${SIGNUP_DOMAIN}`, 'Alice Admin');
  const operator = await signUp(`oscar@${SIGNUP_DOMAIN}`, 'Oscar Operator');
  const outsider = await signUp(`bob@${SIGNUP_DOMAIN}`, 'Bob Outsider');

  const ws = await ok<{ id: string }>('/workspaces', { token: admin.token, body: { name: 'Acme Desk', packs: ['general'] } });
  await ok(`/workspaces/${ws.id}/members`, { token: admin.token, body: { email: operator.email, role: 'operator' } });
  const other = await ok<{ id: string }>('/workspaces', { token: outsider.token, body: { name: 'Other Company', packs: ['general'] } });

  const connectionId = 'conn_e2e_acme';
  const payload = gmailPayload(MAILS);
  await nango('/__e2e/connections', { connection_id: connectionId, organization_id: ws.id, end_user_id: admin.id });
  await nango('/__e2e/mailbox', {
    connection_id: connectionId,
    address: MAILBOX,
    messages: payload.messages,
    attachments: payload.attachments,
  });
  const source = await ok<{ id: string }>(`/workspaces/${ws.id}/sources/nango-connections`, {
    token: admin.token,
    body: { connection_id: connectionId, provider_config_key: 'google-mail' },
  });

  const page = await waitFor('the seeded mail to be read', async () => {
    const res = await call<Page>(`/workspaces/${ws.id}/messages?limit=50&sort=recent`, { token: admin.token });
    const items = res.body.items ?? [];
    return items.length >= MAILS.length && items.every((m) => m.processing_status === 'processed' || m.processing_status === 'failed')
      ? res.body
      : undefined;
  });

  const messages: Record<string, string> = {};
  for (const mail of MAILS) {
    const found = page.items.find((m) => m.subject === mail.subject);
    if (!found) throw new Error(`seeded mail "${mail.subject}" never arrived`);
    messages[mail.key] = found.id;
  }

  await waitFor('attachments to be read', async () => {
    for (const mail of MAILS.filter((m) => m.files?.length)) {
      const res = await call<Detail>(`/workspaces/${ws.id}/messages/${messages[mail.key]}`, { token: admin.token });
      const pending = (res.body.attachments ?? []).some((a) => !a.extract_status || a.extract_status === 'pending');
      if (pending) return undefined;
    }
    return true;
  });

  await call(`/workspaces/${ws.id}/me/agent-profile/skip`, { token: admin.token, method: 'POST' });
  saveSession('admin', admin, [ws.id]);
  saveSession('operator', operator, [ws.id]);
  saveSession('outsider', outsider, [other.id]);

  const seed: Seed = {
    admin,
    operator,
    outsider,
    workspaceId: ws.id,
    outsiderWorkspaceId: other.id,
    sourceId: source.id,
    connectionId,
    messages,
  };
  writeFileSync(SEED_FILE, JSON.stringify(seed, null, 2));
}
