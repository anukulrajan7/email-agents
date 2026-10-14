import type { CSSProperties } from 'react';
import { cn } from '../../lib/cn';
import { useReducedMotion } from '../../lib/useReducedMotion';
import { avatarSrc } from './avatars';

const SIZES = { sm: 'h-8 w-8', md: 'h-14 w-14', lg: 'h-20 w-20', xl: 'h-28 w-28' } as const;

/** Decorative 3D avatar: the agent's name always sits next to it, so the image has no alt text. */
export function AgentFace({
  avatar,
  size = 'md',
  index = 0,
  className,
}: {
  avatar: string | undefined;
  size?: keyof typeof SIZES;
  index?: number;
  className?: string;
}) {
  const reduced = useReducedMotion();
  return (
    <span
      aria-hidden="true"
      data-testid="agent-face"
      data-motion={reduced ? 'still' : 'idle'}
      className={cn('avatar-face', SIZES[size], className)}
      style={{ '--i': index } as CSSProperties}
    >
      {avatar ? <img src={avatarSrc(avatar)} alt="" width={160} height={160} loading="lazy" decoding="async" draggable={false} /> : null}
    </span>
  );
}
