import type { ReactNode } from 'react';

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

type Block = { quoted: boolean; lines: string[] };

function toBlocks(text: string): Block[] {
  const blocks: Block[] = [];
  for (const line of text.split('\n')) {
    const quoted = line.trimStart().startsWith('>');
    const current = blocks[blocks.length - 1];
    if (current && current.quoted === quoted) current.lines.push(line);
    else blocks.push({ quoted, lines: [line] });
  }
  return blocks;
}

export function PlainTextBody({ text }: { text: string }) {
  return (
    <pre className="font-sans text-[15px] leading-relaxed whitespace-pre-wrap break-words text-ink">
      {toBlocks(text).map((block, i) =>
        block.quoted ? (
          <details key={i} className="my-2 border-l-2 border-rule-strong pl-3 text-ink-muted">
            <summary className="cursor-pointer font-semibold hover:text-ink focus-visible:outline-2 focus-visible:outline-accent">
              Show quoted text
            </summary>
            {linkify(block.lines.join('\n'))}
          </details>
        ) : (
          <span key={i}>{linkify(block.lines.join('\n'))}</span>
        ),
      )}
    </pre>
  );
}
