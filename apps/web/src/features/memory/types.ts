import type { Memory, MemoryPatch } from '../../api/types';

export type MemoryOrigin = 'auto' | 'manual';

export type LearnedMemory = Memory & {
  origin?: MemoryOrigin;
  confidence?: number;
  source_message_id?: string | null;
  pinned?: boolean;
  use_count?: number;
  last_used_at?: string | null;
};

export type MemoryUpdate = MemoryPatch & { pinned?: boolean; confirmed?: boolean };

export type MemoryTab = 'all' | 'auto' | 'manual';
export type ConfidenceLevel = 'sure' | 'likely' | 'unsure';
export type GroupKey = 'contacts' | 'clients' | 'answers' | 'preferences';
