import { useParams } from 'react-router';
import { EmptyState } from '../../components/ui';
import { useWorkspace } from '../../app/useWorkspace';
import { MessageDetailView } from './MessageDetailView';
import { GettingStarted } from '../onboarding/GettingStarted';
import { MessageList } from './MessageList';

export function InboxPage() {
  const { ws } = useWorkspace();
  const { messageId } = useParams<{ messageId: string }>();

  return (
    <div className="flex h-full min-h-0">
      <section aria-label="Messages" className="h-full min-h-0 w-[clamp(320px,32vw,400px)] shrink-0 overflow-hidden border-r border-rule-strong bg-surface">
        <MessageList ws={ws} />
      </section>
      <section aria-label="Message detail" className="h-full min-w-0 flex-1 overflow-y-auto">
        {messageId ? (
          <MessageDetailView key={messageId} ws={ws} messageId={messageId} />
        ) : (
          <>
            <GettingStarted ws={ws} />
            <EmptyState title="Select a message" description="Pick an email on the left to see how it was labelled and to draft a reply." />
          </>
        )}
      </section>
    </div>
  );
}
