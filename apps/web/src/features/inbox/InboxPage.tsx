import { Link, useParams } from 'react-router';
import { EmptyState } from '../../components/ui';
import { useWorkspace } from '../../app/useWorkspace';
import { cn } from '../../lib/cn';
import { MessageDetailView } from './MessageDetailView';
import { GettingStarted } from '../onboarding/GettingStarted';
import { MessageList } from './MessageList';

export function InboxPage() {
  const { ws } = useWorkspace();
  const { messageId } = useParams<{ messageId: string }>();
  const open = messageId !== undefined;

  return (
    <div className="flex h-full min-h-0">
      <section
        aria-label="Messages"
        data-guide="inbox-list"
        className={cn(
          'h-full min-h-0 w-full shrink-0 overflow-hidden bg-surface lg:block lg:w-[clamp(320px,30vw,400px)] lg:border-r lg:border-rule-strong',
          open ? 'hidden' : 'block',
        )}
      >
        <MessageList ws={ws} />
      </section>
      <section aria-label="Message detail" data-guide="inbox-reader" className={cn('h-full min-w-0 flex-1 overflow-y-auto [scrollbar-gutter:stable] lg:block', open ? 'block' : 'hidden')}>
        {open ? (
          <>
            <div className="sticky top-0 z-10 flex h-12 items-center border-b border-rule bg-paper/95 px-4 backdrop-blur-sm lg:hidden">
              <Link
                to={`/w/${ws}/inbox`}
                className="-ml-2 inline-flex h-9 items-center gap-1.5 rounded-sm px-2 text-sm font-semibold text-ink hover:bg-ink/5 focus-visible:outline-2 focus-visible:outline-accent"
              >
                <span aria-hidden="true">←</span> Back to inbox
              </Link>
            </div>
            <MessageDetailView key={messageId} ws={ws} messageId={messageId} />
          </>
        ) : (
          <>
            <GettingStarted ws={ws} />
            <EmptyState title="Select a message" description="Pick an email on the left to see what they are asking, how it was labelled, and a draft reply." />
          </>
        )}
      </section>
    </div>
  );
}
