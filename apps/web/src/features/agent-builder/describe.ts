import type { DesignedSpec, SpecStep, SpecTrigger } from './types';

export function describeTrigger(t: SpecTrigger): string {
  if (t.type === 'manual') return 'When you start it yourself';
  if (t.type === 'schedule') return `Every ${t.every_hours} hour${t.every_hours === 1 ? '' : 's'}`;
  const m = t.match;
  const parts: string[] = [];
  if (m.labels.length) parts.push(`labelled ${m.labels.join(' or ')}`);
  if (m.from_domains.length) parts.push(`from ${m.from_domains.join(' or ')}`);
  if (m.from_addresses.length) parts.push(`from ${m.from_addresses.join(' or ')}`);
  if (m.subject_keywords.length) parts.push(`with ${m.subject_keywords.map((k) => `“${k}”`).join(' or ')} in the subject`);
  if (m.needs_reply) parts.push('that need a reply');
  if (m.priority) parts.push(`of ${m.priority} priority`);
  return parts.length ? `When an email arrives ${parts.join(', ')}` : 'When any email arrives';
}

export function describeStep(s: SpecStep): string {
  switch (s.type) {
    case 'extract':
      return `Pull out ${s.fields.map((f) => f.name.replace(/_/g, ' ')).join(', ')}`;
    case 'draft_reply':
      return 'Write a reply draft';
    case 'create_review':
      return `Ask you to review${s.title ? `: ${s.title}` : ''}`;
    case 'remind_if_no_reply':
      return `Remind you if nobody replies in ${s.hours} hours`;
    case 'save_to_gmail_drafts':
      return 'Save the draft in Gmail';
    case 'label':
      return `Label the email ${s.value}`;
    case 'start_case':
      return 'Start a case to follow it through';
    case 'notify':
      return `Let ${s.to === 'admins' ? 'the admins' : s.to.replace(/_/g, ' ')} know`;
  }
}

export function describeSending(p: DesignedSpec['permissions']): string {
  if (p.send === 'never') return 'It never sends anything. It only prepares drafts.';
  if (p.send === 'ask') return 'It asks you before anything is sent.';
  return 'It may send on its own only after it has earned your trust.';
}

export function formatUsd(v: number): string {
  if (v === 0) return '$0';
  return v < 0.01 ? '<$0.01' : `$${v.toFixed(2)}`;
}
