import type { InputHTMLAttributes } from 'react';
import { cn } from '../../lib/cn';
import { fieldClass } from './fieldStyles';

export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  return <input {...rest} className={cn(fieldClass, 'h-10', className)} />;
}
