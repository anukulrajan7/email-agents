import type { AgentQuality, AgentTask, Audience, Formality, Handle, ReplyLength, SendApproval, SendMode, WorkingFor } from './types';

export const QUALITY_OPTIONS: { value: AgentQuality; label: string; hint: string }[] = [
  { value: 'fast', label: 'Fast', hint: 'Quickest and cheapest. Fine for short, routine mail.' },
  { value: 'balanced', label: 'Balanced', hint: 'Good for most mail.' },
  { value: 'best', label: 'Best', hint: 'Most careful writing. Costs more per email.' },
];

export const QUALITY_LABEL: Record<AgentQuality, string> = { fast: 'Fast', balanced: 'Balanced', best: 'Best' };

export const SEND_OPTIONS: { value: SendMode; label: string }[] = [
  { value: 'ask', label: 'Ask first' },
  { value: 'auto', label: 'Routine replies on its own' },
  { value: 'never', label: 'Never send' },
];

export const SEND_SUMMARY: Record<SendMode, string> = {
  ask: 'Asks before sending',
  auto: 'Sends routine replies on its own when the person allows it',
  never: 'Never sends; hands replies to a person',
};

export const TASK_OPTIONS: { value: AgentTask; label: string }[] = [
  { value: 'reply', label: 'Reply to emails' },
  { value: 'requirements', label: 'Work on client requirements' },
];

export const HANDLE_OPTIONS: { value: Handle; label: string }[] = [
  { value: 'client_requirements', label: 'Client requirements' },
  { value: 'candidates', label: 'Candidates' },
  { value: 'timesheets', label: 'Timesheets' },
  { value: 'leave', label: 'Leave' },
  { value: 'payroll', label: 'Payroll' },
  { value: 'onboarding', label: 'Onboarding' },
  { value: 'general', label: 'General questions' },
];

export const AUDIENCE_OPTIONS: { value: Audience; label: string }[] = [
  { value: 'clients', label: 'Clients' },
  { value: 'candidates', label: 'Candidates' },
  { value: 'colleagues', label: 'Colleagues' },
  { value: 'vendors', label: 'Vendors' },
];

export const FORMALITY_OPTIONS: { value: Formality; label: string }[] = [
  { value: '', label: 'No preference' },
  { value: 'casual', label: 'Casual' },
  { value: 'friendly', label: 'Friendly' },
  { value: 'formal', label: 'Formal' },
];

export const LENGTH_OPTIONS: { value: ReplyLength; label: string }[] = [
  { value: '', label: 'No preference' },
  { value: 'short', label: 'Short' },
  { value: 'medium', label: 'Medium' },
  { value: 'detailed', label: 'Detailed' },
];

export const APPROVAL_OPTIONS: { value: SendApproval; label: string }[] = [
  { value: 'always_ask', label: 'Always ask me' },
  { value: 'routine_ok', label: 'Let routine replies go' },
];

export const AVATARS = ['🌼', '🌸', '🪷', '🦉', '🐝', '🌿', '⭐', '💬', '🧭', '🫖'];

export const COLORS = ['ochre', 'postmark', 'sage', 'teal', 'slate', 'plum', 'ink'] as const;

export const COLOR_NAME: Record<string, string> = {
  ochre: 'Ochre',
  postmark: 'Postmark red',
  sage: 'Sage',
  teal: 'Teal',
  slate: 'Slate',
  plum: 'Plum',
  ink: 'Ink',
};

export const MAX_PERSONA = 3000;
export const MAX_SIGNATURE = 200;
export const MAX_NAME = 60;

export const personName = (w: WorkingFor) => w.name ?? w.email;

export function tasksSummary(tasks: AgentTask[], enabled: boolean): string {
  if (!enabled) return 'Switched off';
  if (tasks.length === 0) return 'No jobs switched on';
  return TASK_OPTIONS.filter((t) => tasks.includes(t.value))
    .map((t) => t.label)
    .join(' · ');
}
