import { readFileSync } from 'node:fs';
import path from 'node:path';

const FIXTURES = path.resolve(import.meta.dirname, '../fixtures');
const b64 = (data: string | Buffer) => Buffer.from(data).toString('base64url');

export const XLSX_TYPE = 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet';

export type FixtureFile = { filename: string; mimeType: string; file: string };

export type FixtureMail = {
  key: string;
  from: string;
  subject: string;
  text: string;
  html?: string;
  hoursAgo?: number;
  files?: FixtureFile[];
};

const HOUR = 3_600_000;

/** A Gmail `format=full` message plus the attachment bytes the mock serves for it. */
export function gmailMessage(mail: FixtureMail, mailbox: string, now = Date.now()) {
  const id = `g${mail.key.replace(/[^a-z0-9]/gi, '')}`;
  const files = mail.files ?? [];
  const body: unknown[] = [
    {
      mimeType: 'multipart/alternative',
      parts: [
        { mimeType: 'text/plain', body: { data: b64(mail.text) } },
        ...(mail.html ? [{ mimeType: 'text/html', body: { data: b64(mail.html) } }] : []),
      ],
    },
  ];
  const attachments: Record<string, string> = {};
  files.forEach((f, i) => {
    const attachmentId = `${id}a${i}`;
    const bytes = readFileSync(path.join(FIXTURES, f.file));
    attachments[attachmentId] = b64(bytes);
    body.push({ mimeType: f.mimeType, filename: f.filename, body: { attachmentId, size: bytes.length } });
  });
  const when = now - (mail.hoursAgo ?? 1) * HOUR;
  return {
    message: {
      id,
      threadId: `t${id}`,
      internalDate: String(when),
      labelIds: ['INBOX', 'UNREAD'],
      snippet: mail.text.slice(0, 120),
      payload: {
        mimeType: 'multipart/mixed',
        headers: [
          { name: 'From', value: mail.from },
          { name: 'To', value: mailbox },
          { name: 'Subject', value: mail.subject },
          { name: 'Date', value: new Date(when).toUTCString() },
          { name: 'Message-ID', value: `<${id}@mail.e2e.test>` },
        ],
        parts: body,
      },
    },
    attachments,
  };
}

export const MAILBOX = 'desk@acme-desk.example';

export const MAILS: FixtureMail[] = [
  {
    key: 'java',
    from: 'Anita Rao <anita.rao@capgemini-talent.example>',
    subject: 'Java Developer requirement at Capgemini',
    text: 'Hello team,\n\nWe have an open Java Developer position for our Bengaluru client. The job description is attached.\n\nRegards,\nAnita',
    hoursAgo: 2,
    files: [{ filename: 'job-description.pdf', mimeType: 'application/pdf', file: 'job-description.pdf' }],
  },
  {
    key: 'timesheet',
    from: 'Priya Sharma <priya.sharma@clientco.example>',
    subject: 'September timesheet',
    text: 'Hi team,\n\nPlease find my September timesheet attached. It adds up to 168 hours.\n\nThanks,\nPriya',
    hoursAgo: 3,
    files: [{ filename: 'timesheet-september.xlsx', mimeType: XLSX_TYPE, file: 'timesheet-september.xlsx' }],
  },
  {
    key: 'receipt',
    from: 'Kabir Shah <kabir.shah@clientco.example>',
    subject: 'Cafe receipt for reimbursement',
    text: 'Hi,\n\nThe receipt from the client lunch is attached for reimbursement.\n\nKabir',
    hoursAgo: 4,
    files: [{ filename: 'receipt.png', mimeType: 'image/png', file: 'receipt.png' }],
  },
  {
    key: 'question',
    from: 'Ravi Menon <ravi.menon@buyer.example>',
    subject: 'Question about pricing',
    text: 'Hello,\n\nCould you tell me what your enterprise plan costs? We are a team of 40.\n\nThanks,\nRavi',
    hoursAgo: 5,
  },
  {
    key: 'newsletter',
    from: 'Weekly Digest <digest@news.example>',
    subject: 'Weekly digest for engineers',
    text: 'This week: five articles about databases.',
    html: '<html><head><style>@import url(https://evil.example/x.css);body{font-family:Papyrus}</style></head><body><h1 id="digest-title">Digest title</h1><p>This week: five articles about databases.</p><script>window.__pwned = true; document.title = "pwned";</script></body></html>',
    hoursAgo: 6,
  },
  {
    key: 'reply1',
    from: 'Dev Patel <dev.patel@partner.example>',
    subject: 'Office visit on Friday',
    text: 'Hi,\n\nJust confirming that I will visit your office on Friday afternoon. No action is needed from your side.\n\nDev',
    hoursAgo: 7,
  },
  {
    key: 'reply2',
    from: 'Sana Khan <sana.khan@partner.example>',
    subject: 'Venue booked for the workshop',
    text: 'Hi,\n\nThe venue for the workshop is booked and the invitations go out on Monday.\n\nSana',
    hoursAgo: 8,
  },
  {
    key: 'label1',
    from: 'Neha Verma <neha.verma@vendor.example>',
    subject: 'Invoice INV-1042 for October services',
    text: 'Hello,\n\nPlease find invoice INV-1042 for the October services. The amount due is 1,200 USD.\n\nNeha',
    hoursAgo: 9,
  },
  {
    key: 'urgent',
    from: 'Arjun Nair <arjun.nair@bigclient.example>',
    subject: 'URGENT: production outage needs your reply',
    text: 'Hi,\n\nThis is urgent. Our production system is down and we need an answer from you as soon as possible. Please reply today.\n\nArjun',
    hoursAgo: 28,
  },
];

export function gmailPayload(mails: FixtureMail[], mailbox = MAILBOX) {
  const built = mails.map((m) => gmailMessage(m, mailbox));
  return {
    messages: built.map((b) => b.message),
    attachments: Object.assign({}, ...built.map((b) => b.attachments)) as Record<string, string>,
    ids: Object.fromEntries(mails.map((m, i) => [m.key, built[i]!.message.id])),
  };
}
