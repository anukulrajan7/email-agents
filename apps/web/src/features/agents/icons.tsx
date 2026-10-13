import type { CaseKind, Step } from './types';

const base = {
  'aria-hidden': true,
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.5,
  strokeLinecap: 'round',
  strokeLinejoin: 'round',
  className: 'h-5 w-5 shrink-0',
} as const;

export const KIND_LABEL: Record<CaseKind, string> = { requirement: 'Requirement', inbox: 'Email' };

export function CaseKindIcon({ kind }: { kind: CaseKind }) {
  return kind === 'requirement' ? (
    <svg {...base}>
      <rect x="5" y="3" width="14" height="18" rx="1.5" />
      <path d="M9 8h6M9 12h6M9 16h3" />
    </svg>
  ) : (
    <svg {...base}>
      <rect x="3" y="5" width="18" height="14" rx="1.5" />
      <path d="M3.5 6.5l8.5 7 8.5-7" />
    </svg>
  );
}

export const STEP_LABEL: Record<Step['kind'], string> = {
  tool: 'Action',
  critic: 'Check',
  approval: 'Approval',
  note: 'Note',
  finish: 'Finished',
};

export function StepKindIcon({ kind }: { kind: Step['kind'] }) {
  switch (kind) {
    case 'tool':
      return (
        <svg {...base}>
          <path d="M14.5 5.5a4 4 0 0 0-5 5L4 16l4 4 5.5-5.5a4 4 0 0 0 5-5l-2.5 2.5-2.5-.5-.5-2.5z" />
        </svg>
      );
    case 'critic':
      return (
        <svg {...base}>
          <path d="M12 3l7 3v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6z" />
          <path d="M9 12l2 2 4-4" />
        </svg>
      );
    case 'approval':
      return (
        <svg {...base}>
          <circle cx="12" cy="8" r="3.5" />
          <path d="M5 20c0-4 3-6 7-6s7 2 7 6" />
        </svg>
      );
    case 'note':
      return (
        <svg {...base}>
          <path d="M5 4h14v12l-4 4H5z" />
          <path d="M15 20v-4h4M8 9h8M8 13h4" />
        </svg>
      );
    case 'finish':
      return (
        <svg {...base}>
          <path d="M6 21V4M6 4h12l-2.5 4L18 12H6" />
        </svg>
      );
  }
}
