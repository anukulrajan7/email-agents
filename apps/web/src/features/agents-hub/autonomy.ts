import type { AgentCategory, AgentStatus, Autonomy } from '../../api/types';

export type AutonomyCopy = { chip: string; sentence: string };

export const AUTONOMY: Record<Autonomy, AutonomyCopy> = {
  0: { chip: 'Watching', sentence: 'Watches and learns. Does not touch your mail yet' },
  1: { chip: 'Asks first', sentence: 'Asks before sending' },
  2: { chip: 'Routine replies', sentence: 'Sends routine replies, 10-minute hold' },
  3: { chip: 'Trusted', sentence: 'Replies on its own, 10-minute hold' },
};

export const autonomyLabel = (level: Autonomy) => AUTONOMY[level].sentence;

export const LIMITED_BY: Record<string, string> = {
  earned: 'It has not earned more trust yet',
  admin_cap: 'An admin set a lower limit',
  spec: 'Its job description caps it here',
  workspace: 'The workspace settings cap it here',
  profile: 'Your profile says to always ask first',
  paused: 'Paused',
};

export const STATUS_COPY: Record<AgentStatus, { label: string; tone: 'success' | 'neutral' | 'warning' }> = {
  active: { label: 'Working', tone: 'success' },
  paused: { label: 'Paused', tone: 'warning' },
  draft: { label: 'Draft', tone: 'neutral' },
  archived: { label: 'Archived', tone: 'neutral' },
};

export const CATEGORY_ORDER: AgentCategory[] = ['personal', 'general', 'support', 'sales', 'finance', 'hrms', 'custom'];

export const CATEGORY_TITLE: Record<AgentCategory, string> = {
  personal: 'Personal',
  general: 'Lead',
  support: 'Support',
  sales: 'Sales',
  finance: 'Finance',
  hrms: 'HRMS',
  custom: 'Custom',
};

export const PAUSE_REASON: Record<string, string> = {
  pack_off: 'Its pack is turned off',
  pause_all: 'Everyone is paused',
};
