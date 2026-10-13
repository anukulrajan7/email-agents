import { useEffect, useRef, useState } from 'react';
import { PlainTextBody } from './PlainTextBody';

const MIN_FRAME_HEIGHT = 96;
const MAX_FRAME_HEIGHT = 30_000;

function buildSrcDoc(html: string): string {
  return `<!doctype html><html><head><meta charset="utf-8"><base target="_blank"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src https: data:; style-src 'unsafe-inline'; font-src https: data:"><style>html{overflow-y:hidden}body{margin:0;padding:16px;font:15px/1.6 'Hanken Grotesk Variable',system-ui,sans-serif;color:#1C1B18;word-wrap:break-word} img{max-width:100%;height:auto} a{color:#B73A26} table{max-width:100%!important}</style></head><body>${html}</body></html>`;
}

function HtmlFrame({ html }: { html: string }) {
  const ref = useRef<HTMLIFrameElement>(null);
  const observer = useRef<ResizeObserver | undefined>(undefined);
  const [height, setHeight] = useState(MIN_FRAME_HEIGHT);

  useEffect(() => () => observer.current?.disconnect(), []);

  const measure = () => {
    const doc = ref.current?.contentDocument;
    if (!doc) return;
    const h = doc.body?.offsetHeight || doc.documentElement.offsetHeight;
    if (h) setHeight(Math.min(MAX_FRAME_HEIGHT, Math.max(MIN_FRAME_HEIGHT, Math.ceil(h))));
  };

  const onLoad = () => {
    measure();
    observer.current?.disconnect();
    const body = ref.current?.contentDocument?.body;
    if (body && typeof ResizeObserver !== 'undefined') {
      observer.current = new ResizeObserver(measure);
      observer.current.observe(body);
    }
  };

  return (
    <iframe
      ref={ref}
      title="Email content"
      sandbox="allow-same-origin allow-popups allow-popups-to-escape-sandbox"
      referrerPolicy="no-referrer"
      srcDoc={buildSrcDoc(html)}
      onLoad={onLoad}
      scrolling="no"
      style={{ height }}
      className="block w-full border-0 bg-raised"
    />
  );
}

function OverflowMenu({ plain, onToggle }: { plain: boolean; onToggle: () => void }) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  return (
    <div
      ref={wrapRef}
      className="relative"
      onKeyDown={(e) => {
        if (e.key === 'Escape' && open) {
          e.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        aria-label="More message actions"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
        className="flex h-8 w-8 items-center justify-center rounded-full text-ink-muted hover:bg-ink/5 hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
      >
        <span aria-hidden="true" className="text-lg leading-none font-bold">
          ⋯
        </span>
      </button>
      {open && (
        <div role="menu" className="absolute right-0 z-20 mt-1 min-w-44 border border-rule-strong bg-raised py-1 shadow-[0_8px_24px_-12px_rgba(28,27,24,0.35)]">
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              onToggle();
              setOpen(false);
            }}
            className="block w-full px-4 py-2 text-left text-sm hover:bg-ink/5 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-accent"
          >
            {plain ? 'View formatted' : 'View plain text'}
          </button>
        </div>
      )}
    </div>
  );
}

export function EmailBody({ text, html }: { text: string; html: string | null | undefined }) {
  const [showPlain, setShowPlain] = useState(false);
  const hasHtml = Boolean(html);
  const plain = !hasHtml || showPlain;

  return (
    <div className="flex flex-col gap-1">
      {hasHtml && (
        <div className="flex justify-end">
          <OverflowMenu plain={showPlain} onToggle={() => setShowPlain((v) => !v)} />
        </div>
      )}
      <div role="region" aria-label="Email body" className="overflow-hidden rounded-sm border border-rule bg-raised">
        {plain || !html ? <PlainTextBody text={text} /> : <HtmlFrame html={html} />}
      </div>
      {!plain && <p className="text-xs text-ink-muted">Images load from the sender's servers.</p>}
    </div>
  );
}
