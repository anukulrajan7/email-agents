import { useState } from 'react';
import { cn } from '../../lib/cn';
import { PlainTextBody } from './PlainTextBody';

type View = 'formatted' | 'plain';

const COLLAPSED_HEIGHT = 600;
const EXPANDED_HEIGHT = 1600;

function buildSrcDoc(html: string): string {
  return `<!doctype html><html><head><meta charset="utf-8"><base target="_blank"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src https: data:; style-src 'unsafe-inline'; font-src https: data:"><style>body{margin:0;padding:16px;font:15px/1.6 'Hanken Grotesk Variable',system-ui,sans-serif;color:#1C1B18;word-wrap:break-word} img{max-width:100%;height:auto} a{color:#B73A26} table{max-width:100%!important}</style></head><body>${html}</body></html>`;
}

function ToggleButton({ active, onClick, children }: { active: boolean; onClick: () => void; children: string }) {
  return (
    <button
      type="button"
      aria-pressed={active}
      onClick={onClick}
      className={cn(
        'px-3 py-1 text-sm font-semibold transition-colors focus-visible:outline-2 focus-visible:outline-accent',
        active ? 'bg-ink text-accent-ink' : 'bg-raised text-ink-muted hover:text-ink',
      )}
    >
      {children}
    </button>
  );
}

export function EmailBody({ text, html }: { text: string; html: string | null | undefined }) {
  const [view, setView] = useState<View>('formatted');
  const [expanded, setExpanded] = useState(false);

  if (!html) return <PlainTextBody text={text} />;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div role="group" aria-label="Body format" className="inline-flex divide-x divide-rule overflow-hidden rounded-sm border border-rule">
          <ToggleButton active={view === 'formatted'} onClick={() => setView('formatted')}>
            Formatted
          </ToggleButton>
          <ToggleButton active={view === 'plain'} onClick={() => setView('plain')}>
            Plain text
          </ToggleButton>
        </div>
        {view === 'formatted' && (
          <button
            type="button"
            aria-pressed={expanded}
            onClick={() => setExpanded((v) => !v)}
            className="text-sm font-semibold text-accent underline underline-offset-2 hover:text-accent-hover focus-visible:outline-2 focus-visible:outline-accent"
          >
            {expanded ? 'Collapse' : 'Expand'}
          </button>
        )}
      </div>
      {view === 'formatted' ? (
        <>
          <p className="text-xs text-ink-muted">Images load from the sender's servers.</p>
          <iframe
            title="Email content"
            sandbox="allow-popups allow-popups-to-escape-sandbox"
            referrerPolicy="no-referrer"
            srcDoc={buildSrcDoc(html)}
            style={{ height: expanded ? EXPANDED_HEIGHT : COLLAPSED_HEIGHT }}
            className="w-full rounded-sm border border-rule bg-raised"
          />
        </>
      ) : (
        <PlainTextBody text={text} />
      )}
    </div>
  );
}
