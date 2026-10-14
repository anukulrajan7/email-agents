import type { ReactNode } from 'react';
import { AgentFace } from './AgentFace';

export function FriendlyEmpty({ title, text, avatar = 'sparkles', action }: { title: string; text: string; avatar?: string; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 rounded-sm border border-dashed border-rule-strong px-6 py-8 text-center">
      <AgentFace avatar={avatar} size="lg" />
      <p className="type-display text-xl tracking-[-0.01em]">{title}</p>
      <p className="max-w-sm text-sm text-ink-muted">{text}</p>
      {action}
    </div>
  );
}
