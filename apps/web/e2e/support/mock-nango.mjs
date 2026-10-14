// Stand-in for Nango + the Gmail API behind its proxy, for browser E2E. The API under test talks to this
// exactly as it talks to Nango in production: connect sessions, connection lookup, Gmail list/get/send/drafts.
// Control routes (/__e2e/*) let the test setup load mailboxes and read what the API "sent".
import { createServer } from 'node:http';

const port = Number(process.env.E2E_NANGO_PORT ?? 18081);

/** connection_id -> { organization_id, end_user_id } */
const connections = new Map();
/** connection_id -> { address, messages: Map<id, gmailMessage> } */
const mailboxes = new Map();
/** attachment id -> base64url bytes */
const attachments = new Map();
const sent = [];
const drafts = new Map();
let counter = 0;

const json = (res, status, body) => {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify(body));
};

const readBody = (req) =>
  new Promise((resolve) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      const text = Buffer.concat(chunks).toString('utf8');
      try {
        resolve(text ? JSON.parse(text) : {});
      } catch {
        resolve({});
      }
    });
  });

const decodeRaw = (raw) => {
  const text = Buffer.from(raw ?? '', 'base64url').toString('utf8');
  const [head, ...rest] = text.split(/\r?\n\r\n/);
  const header = (name) => new RegExp(`^${name}:\\s*(.*)$`, 'im').exec(head)?.[1]?.trim() ?? '';
  let body = rest.join('\n\n');
  const encoding = header('Content-Transfer-Encoding').toLowerCase();
  if (encoding === 'base64') body = Buffer.from(body.replace(/\s+/g, ''), 'base64').toString('utf8');
  if (encoding === 'quoted-printable') {
    body = body
      .replace(/=\r?\n/g, '')
      .replace(/=([0-9A-F]{2})/gi, (_, hex) => String.fromCharCode(parseInt(hex, 16)));
  }
  return { to: header('To'), subject: header('Subject'), body, messageId: header('Message-ID'), inReplyTo: header('In-Reply-To') };
};

const mailboxOf = (req) => mailboxes.get(req.headers['connection-id'] ?? '');

createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost');
  const path = url.pathname;
  const method = req.method ?? 'GET';
  const body = method === 'GET' || method === 'DELETE' ? {} : await readBody(req);

  if (path === '/__e2e/health') return json(res, 200, { status: 'ok' });
  if (path === '/__e2e/connections' && method === 'POST') {
    connections.set(body.connection_id, { organization_id: body.organization_id, end_user_id: body.end_user_id });
    return json(res, 200, {});
  }
  if (path === '/__e2e/mailbox' && method === 'POST') {
    const box = mailboxes.get(body.connection_id) ?? { address: body.address, messages: new Map() };
    box.address = body.address ?? box.address;
    for (const m of body.messages ?? []) box.messages.set(m.id, m);
    for (const [id, data] of Object.entries(body.attachments ?? {})) attachments.set(id, data);
    mailboxes.set(body.connection_id, box);
    return json(res, 200, { messages: box.messages.size });
  }
  if (path === '/__e2e/sent') return json(res, 200, { items: sent });
  if (path === '/__e2e/drafts') return json(res, 200, { items: [...drafts.values()] });
  if (path === '/__e2e/reset' && method === 'POST') {
    sent.length = 0;
    drafts.clear();
    return json(res, 200, {});
  }

  if (path === '/connect/sessions' && method === 'POST') {
    return json(res, 201, { data: { token: `nst_${++counter}`, expires_at: new Date(Date.now() + 1_800_000).toISOString() } });
  }
  const conn = /^\/connections\/([^/]+)$/.exec(path);
  if (conn) {
    const id = decodeURIComponent(conn[1]);
    if (method === 'DELETE') {
      connections.delete(id);
      return json(res, 200, {});
    }
    const tags = connections.get(id);
    if (!tags) return json(res, 404, { error: { code: 'not_found' } });
    return json(res, 200, {
      connection_id: id,
      provider_config_key: url.searchParams.get('provider_config_key'),
      provider: 'google-mail',
      tags,
    });
  }
  if (path === '/records') return json(res, 200, { records: [], next_cursor: null });

  const gmail = /^\/proxy\/gmail\/v1\/users\/me\/(.*)$/.exec(path);
  if (gmail) {
    const rest = gmail[1];
    const box = mailboxOf(req);
    if (!box) return json(res, 404, { error: { message: 'unknown connection' } });
    if (rest === 'profile') return json(res, 200, { emailAddress: box.address });
    if (rest === 'messages' && method === 'GET') {
      const query = url.searchParams.get('q') ?? '';
      const rfc = /rfc822msgid:(\S+)/.exec(query)?.[1];
      let list = [...box.messages.values()];
      if (rfc) list = list.filter((m) => JSON.stringify(m.payload?.headers ?? []).includes(rfc));
      list.sort((a, b) => Number(b.internalDate) - Number(a.internalDate));
      return json(res, 200, { messages: list.map((m) => ({ id: m.id, threadId: m.threadId })) });
    }
    const one = /^messages\/([^/?]+)$/.exec(rest);
    if (one && method === 'GET') {
      const message = box.messages.get(one[1]);
      return message ? json(res, 200, message) : json(res, 404, { error: { message: 'not found' } });
    }
    const att = /^messages\/([^/]+)\/attachments\/([^/?]+)$/.exec(rest);
    if (att && method === 'GET') {
      const data = attachments.get(att[2]);
      return data ? json(res, 200, { data, size: Buffer.from(data, 'base64url').length }) : json(res, 404, {});
    }
    if (rest === 'messages/send' && method === 'POST') {
      const id = `sent_${++counter}`;
      sent.push({ id, at: Date.now(), threadId: body.threadId ?? null, ...decodeRaw(body.raw) });
      return json(res, 200, { id, threadId: body.threadId ?? `thr_${counter}` });
    }
    if (rest === 'drafts' && method === 'POST') {
      const id = `draft_${++counter}`;
      drafts.set(id, { id, ...decodeRaw(body.message?.raw) });
      return json(res, 200, { id, message: { id: `dm_${counter}` } });
    }
    const draft = /^drafts\/([^/]+)$/.exec(rest);
    if (draft && method === 'PUT') {
      drafts.set(draft[1], { id: draft[1], ...decodeRaw(body.message?.raw) });
      return json(res, 200, { id: draft[1] });
    }
    if (rest === 'drafts/send' && method === 'POST') {
      const id = `sent_${++counter}`;
      const draftBody = drafts.get(body.id);
      sent.push({ id, at: Date.now(), ...(draftBody ?? {}) });
      return json(res, 200, { id, threadId: `thr_${counter}` });
    }
    if (/^threads\//.test(rest)) return json(res, 200, { messages: [] });
  }
  return json(res, 404, { error: { message: `no mock for ${method} ${path}` } });
}).listen(port, '127.0.0.1', () => console.log(`mock nango listening on ${port}`));
