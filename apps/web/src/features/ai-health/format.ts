export function percent(rate: number): string {
  if (rate === 0) return '0%';
  const value = rate * 100;
  return value < 1 ? '<1%' : `${Math.round(value)}%`;
}

export function seconds(ms: number | null): string {
  if (ms === null) return '-';
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(1)} s`;
}

export const KIND_CLASS: Record<string, string> = {
  leak_blocked: 'fill-danger',
  send_blocked: 'fill-review',
  injection_detected: 'fill-info',
  unsupported_claim: 'fill-accent',
  schema_failure: 'fill-ink-muted',
  refusal: 'fill-success',
  cost_spike: 'fill-ink',
  latency_spike: 'fill-rule-strong',
};
