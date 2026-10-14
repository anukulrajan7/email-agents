import { useId, useState } from 'react';
import type { Attachment } from '../../api/types';
import { Badge, Table, Td, Th } from '../../components/ui';
import { cn } from '../../lib/cn';
import { formatBytes, formatNumber } from '../../lib/format';

function extensionOf(a: Attachment): string {
  const ext = a.name.includes('.') ? a.name.split('.').pop() : undefined;
  if (ext && ext.length <= 4) return ext.toUpperCase();
  const sub = a.media_type?.split('/')[1];
  return (sub ?? 'FILE').slice(0, 4).toUpperCase();
}

const imageKinds: Record<string, string> = {
  document: 'Document',
  screenshot: 'Screenshot',
  photo: 'Photo',
  receipt: 'Receipt',
  id_card: 'ID card',
  other: 'Image',
};

function SheetPreview({ sheets }: { sheets: NonNullable<Attachment['extract_meta']>['sheets'] }) {
  return (
    <div className="flex flex-col gap-3">
      {(sheets ?? []).map((sheet) => {
        const [header = [], ...rows] = sheet.sample;
        return (
          <div key={sheet.name} className="flex flex-col gap-1">
            <p className="text-xs text-ink-muted">
              <span className="font-semibold text-ink">{sheet.name}</span> · {formatNumber(sheet.rows)} rows × {formatNumber(sheet.columns)} columns
              {sheet.truncated && ' · the AI reads the first part'}
            </p>
            <Table aria-label={`${sheet.name} sample`}>
              <thead>
                <tr>
                  {header.map((cell, i) => (
                    <Th key={i}>{cell}</Th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {rows.map((row, r) => (
                  <tr key={r}>
                    {header.map((_, c) => (
                      <Td key={c}>{row[c] ?? ''}</Td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </Table>
          </div>
        );
      })}
    </div>
  );
}

function ImagePreview({ a }: { a: Attachment }) {
  const meta = a.extract_meta ?? {};
  const fields = Object.entries(meta.fields ?? {});
  return (
    <div className="flex flex-col gap-2 text-sm">
      {meta.image_kind && <Badge>{imageKinds[meta.image_kind] ?? 'Image'}</Badge>}
      {meta.summary && <p>{meta.summary}</p>}
      {fields.length > 0 && (
        <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
          {fields.map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-ink-muted">{k}</dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
      )}
      {meta.text && (
        <div>
          <p className="kicker">Text in the image</p>
          <p className="mt-1 max-h-40 overflow-y-auto text-xs whitespace-pre-wrap text-ink-muted">{meta.text}</p>
        </div>
      )}
    </div>
  );
}

function AudioPreview({ a }: { a: Attachment }) {
  const meta = a.extract_meta ?? {};
  const items = meta.action_items ?? [];
  return (
    <div className="flex flex-col gap-2 text-sm">
      {meta.summary && <p>{meta.summary}</p>}
      {items.length > 0 && (
        <div>
          <p className="kicker">Action items</p>
          <ul className="mt-1 list-disc pl-5">
            {items.map((item, i) => (
              <li key={i}>{item}</li>
            ))}
          </ul>
        </div>
      )}
      {meta.truncated && <p className="text-xs text-ink-muted">Only the first part of this recording was transcribed.</p>}
    </div>
  );
}

function Preview({ a }: { a: Attachment }) {
  if (a.extract_kind === 'sheet' && a.extract_meta?.sheets?.length) return <SheetPreview sheets={a.extract_meta.sheets} />;
  if (a.extract_kind === 'image') return <ImagePreview a={a} />;
  if (a.extract_kind === 'audio') return <AudioPreview a={a} />;
  return <p className="text-sm whitespace-pre-wrap">{a.preview}</p>;
}

function Chip({ a }: { a: Attachment }) {
  const [open, setOpen] = useState(false);
  const panelId = useId();
  const readByAi = a.extract_status === 'done';
  const note = !readByAi && a.extract_status && a.extract_kind !== 'unsupported' ? a.extract_note : undefined;
  return (
    <li className={cn('flex max-w-full flex-col gap-2 rounded-sm border border-rule-strong bg-raised px-3 py-2', open ? 'w-full' : 'w-60')}>
      <div className="flex items-center gap-3">
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
            {(a.scan_status === 'pending' || a.scan_status === 'failed') && ` · Scan ${a.scan_status}`}
          </span>
        </span>
      </div>
      {readByAi && (
        <div className="flex items-center justify-between gap-2">
          <Badge tone="info">Read by AI</Badge>
          {a.preview && (
            <button
              type="button"
              aria-expanded={open}
              aria-controls={panelId}
              onClick={() => setOpen((v) => !v)}
              className="text-xs font-semibold text-accent underline decoration-current/40 underline-offset-[3px]"
            >
              {open ? 'Hide preview' : 'Show preview'}
            </button>
          )}
        </div>
      )}
      {note && <p className="text-xs text-ink-muted">{note}</p>}
      {readByAi && open && (
        <div id={panelId} className="border-t border-rule pt-2">
          <Preview a={a} />
        </div>
      )}
    </li>
  );
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
          <Chip key={a.id} a={a} />
        ))}
      </ul>
    </section>
  );
}
