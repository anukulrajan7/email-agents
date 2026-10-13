import type { HTMLAttributes } from 'react';
import { cn } from '../../lib/cn';

export function Page({ className, children, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return (
    <div className="h-full overflow-y-auto [scrollbar-gutter:stable]">
      <div {...rest} className={cn('mx-auto flex w-full max-w-[1440px] flex-col gap-8 px-4 pt-6 pb-12 sm:px-6 lg:px-10 lg:pt-8', className)}>
        {children}
      </div>
    </div>
  );
}
