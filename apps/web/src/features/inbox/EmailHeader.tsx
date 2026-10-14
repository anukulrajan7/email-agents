import { useEffect, useId, useRef, useState } from 'react';
import type { MessageDetail } from '../../api/types';
import { Badge } from '../../components/ui';
import { fullDate } from '../../lib/format';
import { useMe } from '../auth/hooks';
import { LabelStamp } from '../labels/LabelStamp';
import { SeeMatchesLink } from '../matching/SeeMatchesLink';
import { avatarColor, gmailDate, initialOf, parseAddress } from './emailText';
import { cn } from '../../lib/cn';
import { DeleteEmailButton } from './DeleteEmail';

function Avatar({ name, seed }: { name: string; seed: string }) {
  return (
    <span
      aria-hidden="true"
      className={cn('flex h-10 w-10 shrink-0 items-center justify-center rounded-full text-base font-semibold text-accent-ink', avatarColor(seed))}
    >
      {initialOf(name)}
    </span>
  );
}

function recipientSummary(recipients: string[], meEmail: string | undefined): string {
  if (recipients.length === 0) return 'to undisclosed recipients';
  const names = recipients.map((r) => {
    const p = parseAddress(r);
    return meEmail && p.address.toLowerCase() === meEmail.toLowerCase() ? 'me' : p.name;
  });
  const shown = names.slice(0, 2).join(', ');
  return names.length > 2 ? `to ${shown} +${names.length - 2}` : `to ${shown}`;
}

function DetailsPopover({ message }: { message: MessageDetail }) {
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);
  const panelId = useId();

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  const rows: [string, string][] = [['from', message.from]];
  if (message.original_from) rows.push(['original sender (unverified)', message.original_from]);
  rows.push(['to', message.to.join(', ') || 'undisclosed recipients']);
  if (message.cc && message.cc.length > 0) rows.push(['cc', message.cc.join(', ')]);
  rows.push(['date', fullDate(message.received_at)]);

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
        aria-label="Show details"
        aria-expanded={open}
        aria-controls={open ? panelId : undefined}
        onClick={() => setOpen((v) => !v)}
        className="flex h-5 w-5 items-center justify-center rounded-sm text-[10px] text-ink-muted hover:bg-ink/5 hover:text-ink focus-visible:outline-2 focus-visible:outline-accent"
      >
        <span aria-hidden="true">▼</span>
      </button>
      {open && (
        <div
          id={panelId}
          role="group"
          aria-label="Message details"
          className="absolute top-full left-0 z-20 mt-1 w-[min(28rem,80vw)] border border-rule-strong bg-raised p-4 shadow-[0_8px_24px_-12px_rgba(28,27,24,0.35)]"
        >
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-sm">
            {rows.map(([k, v]) => (
              <div key={k} className="contents">
                <dt className="text-right text-ink-muted">{k}:</dt>
                <dd className="break-all">{v}</dd>
              </div>
            ))}
          </dl>
        </div>
      )}
    </div>
  );
}

export function EmailHeader({ ws, message }: { ws: string; message: MessageDetail }) {
  const me = useMe().data?.user.email;
  const sender = parseAddress(message.from);
  const name = message.contact?.display_name || sender.name;
  const category = message.classification?.category;

  return (
    <header className="flex flex-col gap-4">
      <div className="flex flex-col gap-2">
        <div className="flex items-start justify-between gap-3">
          <h2 className="text-[22px] leading-tight font-normal break-words text-ink sm:text-[26px]">{message.subject || '(no subject)'}</h2>
          <DeleteEmailButton ws={ws} messageId={message.id} />
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {category && <LabelStamp ws={ws} category={category} />}
          {message.classification?.priority === 'high' && <Badge tone="danger">High priority</Badge>}
          {message.processing_status === 'failed' && <Badge tone="danger">Processing failed</Badge>}
          {(category === 'client_requirement' || category === 'candidate_referral') && (
            <SeeMatchesLink ws={ws} messageId={message.id} category={category} />
          )}
        </div>
      </div>
      <div className="flex items-start gap-3">
        <Avatar name={name} seed={sender.address} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-0.5">
            <p className="min-w-0 text-sm break-words">
              <span className="font-bold">{name}</span> <span className="text-ink-muted">&lt;{sender.address}&gt;</span>
            </p>
            <time dateTime={message.received_at} title={fullDate(message.received_at)} className="shrink-0 text-xs text-ink-muted">
              {gmailDate(message.received_at)}
            </time>
          </div>
          <div className="flex items-center gap-1 text-xs text-ink-muted">
            <span className="min-w-0 truncate">{recipientSummary(message.to, me)}</span>
            <DetailsPopover message={message} />
          </div>
        </div>
      </div>
    </header>
  );
}
