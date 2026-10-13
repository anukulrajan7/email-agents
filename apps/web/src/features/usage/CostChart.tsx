import { shortDate, usd } from './format';
import type { DayUsage } from './types';

const WIDTH = 600;
const HEIGHT = 140;
const GAP = 3;

export function CostChart({ days }: { days: DayUsage[] }) {
  const max = Math.max(...days.map((d) => d.usd), 0);
  const slot = WIDTH / Math.max(days.length, 1);
  const first = days[0];
  const last = days[days.length - 1];

  return (
    <figure className="flex flex-col gap-2">
      <svg
        viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
        role="img"
        aria-label={`Cost per day for the last ${days.length} days`}
        className="h-36 w-full"
        preserveAspectRatio="none"
      >
        <line x1="0" y1={HEIGHT - 0.5} x2={WIDTH} y2={HEIGHT - 0.5} className="stroke-rule-strong" strokeWidth="1" vectorEffect="non-scaling-stroke" />
        {days.map((d, i) => {
          const h = max > 0 && d.usd > 0 ? Math.max(2, (d.usd / max) * (HEIGHT - 6)) : 0;
          return (
            <rect key={d.date} x={i * slot + GAP / 2} y={HEIGHT - 1 - h} width={Math.max(slot - GAP, 1)} height={h} className="fill-accent">
              <title>{`${shortDate(d.date)}: ${usd(d.usd)}, ${d.calls} ${d.calls === 1 ? 'call' : 'calls'}`}</title>
            </rect>
          );
        })}
      </svg>
      {first && last && (
        <figcaption className="flex justify-between font-mono text-xs text-ink-muted">
          <span>{shortDate(first.date)}</span>
          <span>Peak day {usd(max)}</span>
          <span>{shortDate(last.date)}</span>
        </figcaption>
      )}
    </figure>
  );
}
