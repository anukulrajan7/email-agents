import type { AgentCategory, AgentSummary } from '../../api/types';

export type AvatarChoice = { key: string; label: string };

/** Microsoft Fluent Emoji (3D), MIT licensed. Files live in /public/avatars. */
export const AVATARS: AvatarChoice[] = [
  { key: 'blossom', label: 'Blossom' },
  { key: 'owl', label: 'Owl' },
  { key: 'bee', label: 'Bee' },
  { key: 'dolphin', label: 'Dolphin' },
  { key: 'fox', label: 'Fox' },
  { key: 'panda', label: 'Panda' },
  { key: 'koala', label: 'Koala' },
  { key: 'robot', label: 'Robot' },
  { key: 'rocket', label: 'Rocket' },
  { key: 'sparkles', label: 'Sparkles' },
  { key: 'penguin', label: 'Penguin' },
  { key: 'otter', label: 'Otter' },
  { key: 'hedgehog', label: 'Hedgehog' },
  { key: 'rabbit', label: 'Rabbit' },
  { key: 'cat', label: 'Cat' },
  { key: 'turtle', label: 'Turtle' },
];

const PREFIX = 'fluent:';
const KEYS = new Set(AVATARS.map((a) => a.key));

export const avatarValue = (key: string) => `${PREFIX}${key}`;

export function parseAvatar(value: string | undefined): string | undefined {
  if (!value?.startsWith(PREFIX)) return undefined;
  const key = value.slice(PREFIX.length);
  return KEYS.has(key) ? key : undefined;
}

export const avatarLabel = (key: string) => AVATARS.find((a) => a.key === key)?.label ?? key;

export const avatarSrc = (key: string) => `/avatars/${key}.webp`;

const BY_TEMPLATE: Record<string, string> = {
  'general.lead': 'blossom',
  'general.inbox': 'sparkles',
  'support.resolver': 'dolphin',
  'support.escalation': 'penguin',
  'sales.qualifier': 'bee',
  'sales.followups': 'rabbit',
  'finance.invoices': 'owl',
  'personal.scheduling': 'rocket',
  'personal.mailbox': 'panda',
  'hrms.leave': 'turtle',
  'hrms.timesheets': 'hedgehog',
  'hrms.onboarding': 'koala',
  'hrms.requirements': 'otter',
  'hrms.payroll': 'cat',
};

const BY_CATEGORY: Record<AgentCategory, string> = {
  general: 'sparkles',
  support: 'dolphin',
  sales: 'bee',
  finance: 'owl',
  personal: 'fox',
  hrms: 'otter',
  custom: 'robot',
};

type AvatarSource = Pick<AgentSummary, 'avatar' | 'kind' | 'template_key' | 'category' | 'scope'>;

/** The picked avatar if there is one, otherwise the template's default. */
export function resolveAvatar(agent: AvatarSource | { avatar?: string }): string | undefined {
  const picked = parseAvatar(agent.avatar);
  if (picked) return picked;
  if (!('kind' in agent)) return undefined;
  if (agent.kind === 'lead') return 'blossom';
  if (agent.template_key && BY_TEMPLATE[agent.template_key]) return BY_TEMPLATE[agent.template_key];
  if (agent.scope === 'personal') return 'panda';
  return BY_CATEGORY[agent.category];
}
