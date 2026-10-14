import { shortDate } from '../usage/format';
import { KIND_CLASS } from './format';
import type { DayIncidents } from './types';

const WIDTH = 600;
const HEIGHT = 140;
const GAP = 3;

export function IncidentChart({ days, kinds }: { days: DayIncidents[]; kinds: string[] }) {
  const max = Math.max(...days.map((d) => d.total), 0);
  const slot = WIDTH / Math.max(days.length, 1);
  const first = days[0];
  const last = days[days.length - 1];

  return (
    <figure className="flex flex-col gap-2">
      <svg
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="img"
        aria-label={`Incidents per day for the last ${days.length} days`}
        className="h-36 w-full"
        preserveAspectRatio="none"
      >
        <line x1="0" y1={HEIGHT - 0.5} x2={WIDTH} y2={HEIGHT - 0.5} className="stroke-rule-strong" strokeWidth="1" vectorEffect="non-scaling-stroke" />
        {days.map((d, i) => {
          let y = HEIGHT - 1;
          return (
            <g key={d.date}>
              <title>{`${shortDate(d.date)}: ${d.total} ${d.total === 1 ? 'incident' : 'incidents'}`}</title>
              {kinds.map((kind) => {
                const n = d.counts[kind] ?? 0;
                if (n === 0 || max === 0) return null;
                const h = Math.max(2, (n / max) * (HEIGHT - 6));
                y -= h;
                return <rect key={kind} x={i * slot + GAP / 2} y={y} width={Math.max(slot - GAP, 1)} height={h} className={KIND_CLASS[kind] ?? 'fill-accent'} />;
              })}
            </g>
          );
        })}
      </svg>
      {first && last && (
        <figcaption className="flex justify-between font-mono text-xs text-ink-muted">
          <span>{shortDate(first.date)}</span>
          <span>Worst day {max}</span>
          <span>{shortDate(last.date)}</span>
        </figcaption>
      )}
    </figure>
  );
}
