import { useState, type ReactNode } from 'react';
import { splitEmailText, type TextSegment } from './emailText';

const TOKEN_RE = /https?:\/\/[^\s<>"]+|[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)+/g;
const TRAILING_PUNCT_RE = /[.,;:!?)\]}'"]+$/;
const LINK_CLASS = 'break-all text-accent underline underline-offset-2 hover:text-accent-hover';

function linkify(text: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  for (const match of text.matchAll(TOKEN_RE)) {
    const start = match.index;
    let token = match[0];
    const trailing = token.match(TRAILING_PUNCT_RE)?.[0] ?? '';
    token = token.slice(0, token.length - trailing.length);
    if (!token) continue;
    if (start > last) nodes.push(text.slice(last, start));
    const href = token.includes('://') ? token : `mailto:${token}`;
    nodes.push(
      <a key={start} href={href} target="_blank" rel="noopener noreferrer nofollow" className={LINK_CLASS}>
        {token}
      </a>,
    );
    last = start + token.length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function Trimmed({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  return (
    <div className="my-1">
      <button
        type="button"
        aria-expanded={open}
        aria-label={open ? 'Hide trimmed content' : 'Show trimmed content'}
        onClick={() => setOpen((v) => !v)}
        className="inline-flex h-5 items-center rounded-sm border border-rule-strong bg-neutral-tint px-2 text-xs leading-none font-bold tracking-widest text-ink-muted hover:border-ink hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
      >
        <span aria-hidden="true">•••</span>
      </button>
      {open && <div className="mt-2 border-l-2 border-rule-strong pl-3 text-ink-muted">{linkify(text)}</div>}
    </div>
  );
}

function Segment({ segment }: { segment: TextSegment }) {
  if (segment.kind === 'quoted') return <Trimmed text={segment.text} />;
  if (segment.kind === 'signature') return <div className="mt-4 text-ink-muted">{linkify(segment.text)}</div>;
  return <div className="mb-1">{linkify(segment.text)}</div>;
}

export function PlainTextBody({ text }: { text: string }) {
  const segments = splitEmailText(text);
  return (
    <div className="p-4 font-sans text-[15px] leading-[1.6] break-words whitespace-pre-wrap text-ink">
      {segments.length === 0 ? <span className="text-ink-muted">(No message text)</span> : segments.map((s, i) => <Segment key={i} segment={s} />)}
    </div>
  );
}
