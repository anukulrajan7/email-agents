import type { MessageDetail } from '../../api/types';
import { AttachmentChips } from './AttachmentChips';
import { EmailBody } from './EmailBody';
import { EmailHeader } from './EmailHeader';
import { InsightCard } from './InsightCard';

export function EmailReader({ ws, message }: { ws: string; message: MessageDetail }) {
  return (
    <article aria-label="Email" className="flex flex-col gap-5">
      <EmailHeader ws={ws} message={message} />
      <InsightCard ws={ws} messageId={message.id} insight={message.insight} status={message.processing_status} replied={message.replied ?? false} />
      <EmailBody text={message.body_text} html={message.body_html_safe} />
      <AttachmentChips attachments={message.attachments} />
    </article>
  );
}
