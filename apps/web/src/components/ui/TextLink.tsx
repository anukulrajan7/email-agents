import { Link, type LinkProps } from 'react-router';
import { cn } from '../../lib/cn';

export const linkClass =
  'rounded-xs text-accent underline decoration-accent/40 underline-offset-[3px] hover:text-accent-hover hover:decoration-accent-hover focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent';

export function TextLink({ className, ...rest }: LinkProps) {
  return <Link {...rest} className={cn(linkClass, className)} />;
}
