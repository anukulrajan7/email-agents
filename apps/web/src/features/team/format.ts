import { formatPercent } from '../../lib/format';
import type { MetricUnit, MetricValue } from './types';

export function formatMetric(value: number | null | undefined, unit: MetricUnit): string {
  if (value === null || value === undefined) return '–';
  if (unit === 'rate') return formatPercent(value);
  if (unit === 'minutes') return value >= 120 ? `${Math.round(value / 60)} h` : `${Math.round(value)} min`;
  return String(Math.round(value));
}

export const ofTarget = (value: number, target: number | null) => (target ? `${value}/${target}` : String(value));

/** The one quality number worth showing next to the target: the first non-primary metric with a value. */
export function qualityMetric(metrics: MetricValue[]): MetricValue | undefined {
  const preferred = metrics.find((m) => m.key === 'interview_rate') ?? metrics.find((m) => !m.primary && m.unit !== 'count');
  return preferred;
}

export function progress(value: number, target: number | null): number {
  if (!target) return 0;
  return Math.min(100, Math.round((value / target) * 100));
}
