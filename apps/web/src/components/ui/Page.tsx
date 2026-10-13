import type { HTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export function Page({ className, children, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div className="h-full overflow-y-auto [scrollbar-gutter:stable]">
      <div {...rest} className={cn('flex flex-col gap-8 px-8 pt-6 pb-10', className)}>
        {children}
      </div>
    </div>
  );
}
