import type { TextareaHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';
import { fieldClass } from './fieldStyles';

export function Textarea({ className, ...rest }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea {...rest} className={cn(fieldClass, 'py-2', className)} />;
}
