import { twMerge } from 'tailwind-merge';

export const cn = (...parts: Array<string | false | null | undefined>) => twMerge(parts.filter(Boolean).join(' '));
