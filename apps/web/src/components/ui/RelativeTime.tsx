import { cn } from '../../lib/cn';
import { fullDate, relativeTime } from '../../lib/format';

export function RelativeTime({ iso, prefix, className }: { iso: string; prefix?: string; className?: string }) {
  return (
    <time dateTime={iso} title={fullDate(iso)} className={cn('whitespace-nowrap', className)}>
      {prefix}
      {relativeTime(iso)}
    </time>
  );
}
