import type { ConfidenceLevel, GroupKey, LearnedMemory, MemoryOrigin, MemoryTab } from './types';

export const UNSURE_BELOW = 0.6;
const SURE_FROM = 0.85;

export const GROUPS: { key: GroupKey; title: string; hint: string }[] = [
  { key: 'contacts', title: 'Contacts', hint: 'Who people are and how they like to work with you.' },
  { key: 'clients', title: 'Clients', hint: 'What the assistant knows about the companies you serve.' },
  { key: 'answers', title: 'Standing answers', hint: 'Answers your team gives again and again.' },
  { key: 'preferences', title: 'Team preferences', hint: 'How your team likes replies to read.' },
];

export function groupOf(m: LearnedMemory): GroupKey {
  const kind = String(m.kind).toLowerCase();
  if (kind.includes('client')) return 'clients';
  if (kind.includes('answer') || kind === 'faq') return 'answers';
  if (kind.includes('contact') || m.scope === 'contact') return 'contacts';
  if (kind === 'preference' || kind === 'style') return 'preferences';
  return 'answers';
}

export function originOf(m: LearnedMemory): MemoryOrigin {
  return m.origin ?? (m.source === 'learned' ? 'auto' : 'manual');
}

export function confidenceOf(m: LearnedMemory): number {
  if (m.confidence !== undefined) return m.confidence;
  if (m.status === 'suggested') return 0.4;
  return originOf(m) === 'manual' ? 1 : 0.8;
}

export function confidenceLevel(m: LearnedMemory): ConfidenceLevel {
  const c = confidenceOf(m);
  if (c < UNSURE_BELOW) return 'unsure';
  return c >= SURE_FROM ? 'sure' : 'likely';
}

export function needsConfirmation(m: LearnedMemory): boolean {
  return originOf(m) === 'auto' && confidenceLevel(m) === 'unsure';
}

export function matchesTab(m: LearnedMemory, tab: MemoryTab): boolean {
  return tab === 'all' || originOf(m) === tab;
}

function compare(a: LearnedMemory, b: LearnedMemory): number {
  if (Boolean(a.pinned) !== Boolean(b.pinned)) return a.pinned ? -1 : 1;
  const used = (b.use_count ?? 0) - (a.use_count ?? 0);
  if (used !== 0) return used;
  return b.created_at.localeCompare(a.created_at);
}

export function groupMemories(items: LearnedMemory[]): { key: GroupKey; title: string; hint: string; items: LearnedMemory[] }[] {
  return GROUPS.map((g) => ({ ...g, items: items.filter((m) => groupOf(m) === g.key).sort(compare) })).filter((g) => g.items.length > 0);
}
