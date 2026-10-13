import { useParams } from 'react-router';
import { EmptyState } from '../../components/ui';
import { useWorkspace } from '../../app/useWorkspace';
import { MessageDetailView } from './MessageDetailView';
import { MessageList } from './MessageList';

export function InboxPage() {
  const { ws } = useWorkspace();
  const { messageId } = useParams<{ messageId: string }>();

  return (
    <div className="flex h-full">
      <section aria-label="Messages" className="w-[clamp(320px,32vw,400px)] shrink-0 border-r border-rule-strong bg-surface">
        <MessageList ws={ws} />
      </section>
      <section aria-label="Message detail" className="min-w-0 flex-1 overflow-y-auto">
        {messageId ? (
          <MessageDetailView key={messageId} ws={ws} messageId={messageId} />
        ) : (
          <EmptyState title="Select a message" description="Pick an email on the left to see its details and draft a reply." />
        )}
      </section>
    </div>
  );
}
