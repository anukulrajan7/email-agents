import type { SelectHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';
import { fieldClass } from './fieldStyles';

export function Select({ className, ...rest }: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select {...rest} className={cn(fieldClass, 'select-chevron h-10', className)} />;
}
