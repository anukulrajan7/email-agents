import { readFileSync } from 'node:fs';
import { BASE_URL, MAIL_FILE, NANGO_URL, PASSWORD, SEED_FILE } from './env';

export type Session = { token: string; id: string; email: string };

export type Seed = {
  admin: Session;
  operator: Session;
  outsider: Session;
  workspaceId: string;
  outsiderWorkspaceId: string;
  sourceId: string;
  connectionId: string;
  messages: Record<string, string>;
};

export async function call<T = unknown>(
  path: string,
  opts: { method?: string; token?: string; body?: unknown } = {},
): Promise<{ status: number; body: T }> {
  const res = await fetch(`${BASE_URL}/api/v1${path}`, {
    method: opts.method ?? (opts.body === undefined ? 'GET' : 'POST'),
    headers: {
      Accept: 'application/json',
      ...(opts.body === undefined ? {} : { 'Content-Type': 'application/json' }),
      ...(opts.token ? { 'X-Auth-Token': opts.token } : {}),
    },
    body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
  });
  const text = await res.text();
  let body: unknown = undefined;
  try {
    body = text ? JSON.parse(text) : undefined;
  } catch {
    body = text;
  }
  return { status: res.status, body: body as T };
}

export async function ok<T = unknown>(path: string, opts: Parameters<typeof call>[1] = {}): Promise<T> {
  const { status, body } = await call<T>(path, opts);
  if (status >= 300) throw new Error(`${opts.method ?? 'GET'} ${path} -> ${status} ${JSON.stringify(body)}`);
  return body;
}

export function mailTo(address: string): { to: string; subject: string; text: string }[] {
  let raw = '';
  try {
    raw = readFileSync(MAIL_FILE, 'utf8');
  } catch {
    return [];
  }
  return raw
    .split('\n')
    .filter(Boolean)
    .map((line) => JSON.parse(line) as { to: string; subject: string; text: string })
    .filter((m) => m.to.toLowerCase() === address.toLowerCase());
}

/** The newest 6-digit code mailed to `address` (the captured verification email). */
export async function otpFor(address: string, after = 0): Promise<string> {
  const deadline = Date.now() + 15_000;
  for (;;) {
    const mails = mailTo(address).filter((m) => /\b\d{6}\b/.test(m.text));
    if (mails.length > after) return /\b(\d{6})\b/.exec(mails[mails.length - 1]!.text)![1]!;
    if (Date.now() > deadline) throw new Error(`no verification code was mailed to ${address}`);
    await new Promise((r) => setTimeout(r, 200));
  }
}

export async function signUp(email: string, name?: string): Promise<Session> {
  const before = mailTo(email).length;
  const res = await call('/auth/signup', { body: { email, password: PASSWORD, name } });
  if (res.status !== 202) throw new Error(`signup ${email} -> ${res.status} ${JSON.stringify(res.body)}`);
  const code = await otpFor(email, before);
  const session = await ok<{ token: string; user: { id: string; email: string } }>('/auth/verify', { body: { email, code } });
  return { token: session.token, id: session.user.id, email };
}

export async function nango<T = unknown>(path: string, body?: unknown): Promise<T> {
  const res = await fetch(`${NANGO_URL}${path}`, {
    method: body === undefined ? 'GET' : 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return (await res.json()) as T;
}

export type SentMail = { id: string; to: string; subject: string; body: string; at: number; threadId: string | null; inReplyTo: string };
export const sentMail = async () => (await nango<{ items: SentMail[] }>('/__e2e/sent')).items;

/** A throwaway workspace owned by `owner`, so specs that change settings never touch the seeded one. */
export async function createWorkspace(owner: Session, name: string, packs: string[] = ['general'], members: Session[] = []) {
  const ws = await ok<{ id: string }>('/workspaces', { token: owner.token, body: { name, packs } });
  for (const m of members) await ok(`/workspaces/${ws.id}/members`, { token: owner.token, body: { email: m.email, role: 'operator' } });
  return ws.id;
}

export function seed(): Seed {
  return JSON.parse(readFileSync(SEED_FILE, 'utf8')) as Seed;
}
