import { relativeTime } from '../../lib/format';

const HTML_COMMENT_RE = /<!--[\s\S]*?-->/g;
const CONDITIONAL_RE = /<!\[(?:endif|if[^\]]*)\]-->|<!--\[(?:endif|if[^\]]*)\]>|<!\[(?:endif|if[^\]]*)\]>/gi;
const ANGLE_URL_RE = /<((?:https?:\/\/)[^\s<>]+)>/g;

export function cleanEmailText(text: string): string {
  return text
    .replace(/\r\n?/g, '\n')
    .replace(HTML_COMMENT_RE, '')
    .replace(CONDITIONAL_RE, '')
    .replace(ANGLE_URL_RE, '$1')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

export type TextSegment = { kind: 'text' | 'quoted' | 'signature'; text: string };

const WROTE_LINE_RE = /^\s*On\s.+\swrote:\s*$/i;
const WROTE_TAIL_RE = /^\s*(?:.{0,80}\s)?wrote:\s*$/i;
const ORIGINAL_MESSAGE_RE = /^\s*-{2,}\s*Original Message\s*-{2,}\s*$/i;
const isQuoteLine = (line: string) => line.trimStart().startsWith('>');
const isSignatureDelimiter = (line: string) => line.trimEnd() === '--';

function findHistoryStart(lines: string[]): number {
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i] ?? '';
    if (ORIGINAL_MESSAGE_RE.test(line)) return i;
    if (WROTE_LINE_RE.test(line)) return i;
    const next = lines[i + 1];
    if (/^\s*On\s/i.test(line) && next !== undefined && WROTE_TAIL_RE.test(next)) return i;
  }
  return -1;
}

const joinTrimmed = (lines: string[]) => lines.join('\n').replace(/^\n+|\s+$/g, '');

function pushSegment(out: TextSegment[], kind: TextSegment['kind'], lines: string[]) {
  const text = joinTrimmed(lines);
  if (text) out.push({ kind, text });
}

export function splitEmailText(raw: string): TextSegment[] {
  const lines = cleanEmailText(raw).split('\n');
  const marker = findHistoryStart(lines);
  const hasLeadText = marker > 0 && lines.slice(0, marker).some((l) => l.trim() !== '');
  const head = hasLeadText ? lines.slice(0, marker) : lines;
  const history = hasLeadText ? lines.slice(marker) : [];

  const sigAt = head.findIndex((l) => isSignatureDelimiter(l));
  const body = sigAt === -1 ? head : head.slice(0, sigAt);
  const signature = sigAt === -1 ? [] : head.slice(sigAt);

  const out: TextSegment[] = [];
  let kind: 'text' | 'quoted' = 'text';
  let current: string[] = [];
  body.forEach((line, i) => {
    let lineKind: 'text' | 'quoted' = isQuoteLine(line) ? 'quoted' : 'text';
    if (line.trim() === '' && kind === 'quoted') {
      const nextLine = body.slice(i + 1).find((l) => l.trim() !== '');
      if (nextLine !== undefined && isQuoteLine(nextLine)) lineKind = 'quoted';
    }
    if (lineKind !== kind) {
      pushSegment(out, kind, current);
      current = [];
      kind = lineKind;
    }
    current.push(line);
  });
  pushSegment(out, kind, current);
  pushSegment(out, 'signature', signature);
  pushSegment(out, 'quoted', history);
  return out;
}

export type ParsedAddress = { name: string; address: string };

const ADDRESS_RE = /^\s*"?([^"<]*?)"?\s*<([^<>]+)>\s*$/;

export function parseAddress(raw: string): ParsedAddress {
  const m = ADDRESS_RE.exec(raw);
  const address = (m?.[2] ?? raw).trim();
  const name = (m?.[1] ?? '').trim() || address.split('@')[0] || address;
  return { name, address };
}

export function initialOf(name: string): string {
  const match = /[\p{L}\p{N}]/u.exec(name);
  return (match?.[0] ?? '?').toUpperCase();
}

const AVATAR_COLORS = [
  'bg-info',
  'bg-success',
  'bg-plum',
  'bg-payroll',
  'bg-review-ink',
  'bg-accent',
  'bg-ink',
  'bg-accent-hover',
] as const;

export function avatarColor(seed: string): string {
  let hash = 0;
  for (const ch of seed.toLowerCase()) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0;
  return AVATAR_COLORS[hash % AVATAR_COLORS.length] ?? AVATAR_COLORS[0];
}

export function gmailDate(iso: string, now = Date.now()): string {
  const d = new Date(iso);
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  const base = d.toLocaleString('en-US', {
    month: 'short',
    day: 'numeric',
    ...(sameYear ? {} : { year: 'numeric' }),
    hour: 'numeric',
    minute: '2-digit',
  });
  return `${base} (${relativeTime(iso, now)})`;
}
