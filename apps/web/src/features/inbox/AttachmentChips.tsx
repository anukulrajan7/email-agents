import type { Attachment } from '../../api/types';
import { formatBytes } from '../../lib/format';

function extensionOf(a: Attachment): string {
  const ext = a.name.includes('.') ? a.name.split('.').pop() : undefined;
  if (ext && ext.length <= 4) return ext.toUpperCase();
  const sub = a.media_type?.split('/')[1];
  return (sub ?? 'FILE').slice(0, 4).toUpperCase();
}

export function AttachmentChips({ attachments }: { attachments: Attachment[] }) {
  if (attachments.length === 0) return null;
  return (
    <section aria-label="Attachments" className="flex flex-col gap-2">
      <h3 className="text-sm font-semibold">
        {attachments.length} {attachments.length === 1 ? 'attachment' : 'attachments'}
      </h3>
      <ul className="flex flex-wrap gap-3">
        {attachments.map((a) => (
          <li key={a.id} className="flex w-60 max-w-full items-center gap-3 rounded-sm border border-rule-strong bg-raised px-3 py-2">
            <span
              aria-hidden="true"
              className="flex h-10 w-10 shrink-0 items-center justify-center rounded-sm bg-neutral-tint font-mono text-[11px] font-bold text-ink-muted"
            >
              {extensionOf(a)}
            </span>
            <span className="min-w-0 text-sm">
              <span className="block truncate font-semibold" title={a.name}>
                {a.name}
              </span>
              <span className="block truncate text-xs text-ink-muted">
                {formatBytes(a.size_bytes)}
                {a.scan_status !== 'clean' && ` · Scan ${a.scan_status}`}
              </span>
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
