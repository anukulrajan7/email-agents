import { shortDate } from './format';
import type { SeriesPoint } from './types';

const WIDTH = 600;
const HEIGHT = 120;

interface Props {
  title: string;
  points: SeriesPoint[];
  pick: (p: SeriesPoint) => number | null;
  format: (v: number | null) => string;
  max?: number;
}

export function TrendChart({ title, points, pick, format, max }: Props) {
  const values = points.map(pick);
  const known = values.filter((v): v is number => v !== null);
  const top = max ?? Math.max(...known, 0.0001);
  const step = WIDTH / Math.max(points.length - 1, 1);
  const segments: string[] = [];
  let current: string[] = [];
  values.forEach((v, i) => {
    if (v === null) {
      if (current.length > 0) segments.push(current.join(' '));
      current = [];
      return;
    }
    current.push(`${i * step},${HEIGHT - 4 - (v / top) * (HEIGHT - 8)}`);
  });
  if (current.length > 0) segments.push(current.join(' '));
  const first = points[0];
  const last = points[points.length - 1];
  const latest = known[known.length - 1] ?? null;

  return (
    <figure className="flex flex-col gap-2">
      <figcaption className="flex items-baseline justify-between">
        <span className="kicker">{title}</span>
        <span className="font-mono text-sm text-ink">{known.length > 0 ? `Latest ${format(latest)}` : 'No data yet'}</span>
      </figcaption>
      <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} role="img" aria-label={`${title} over the last ${points.length} days`} className="h-28 w-full" preserveAspectRatio="none">
        <line x1="0" y1={HEIGHT - 0.5} x2={WIDTH} y2={HEIGHT - 0.5} className="stroke-rule-strong" strokeWidth="1" vectorEffect="non-scaling-stroke" />
        {segments.map((s) =>
          s.includes(' ') ? (
            <polyline key={s} points={s} fill="none" className="stroke-accent" strokeWidth="2" vectorEffect="non-scaling-stroke" />
          ) : (
            <circle key={s} cx={s.split(',')[0]} cy={s.split(',')[1]} r="2.5" className="fill-accent" />
          ),
        )}
      </svg>
      {first && last && (
        <div className="flex justify-between font-mono text-xs text-ink-muted">
          <span>{shortDate(first.date)}</span>
          <span>{shortDate(last.date)}</span>
        </div>
      )}
    </figure>
  );
}
