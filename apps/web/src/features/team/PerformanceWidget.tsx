import { Link } from 'react-router';
import { cn } from '../../lib/cn';
import { formatMetric, ofTarget, progress, qualityMetric } from './format';
import { useMyMetrics } from './hooks';

/** The corner widget: your numbers against your targets. Only people in the team directory see it. */
export function PerformanceWidget({ ws }: { ws: string }) {
  const mine = useMyMetrics(ws);
  const m = mine.data?.metrics;
  if (!m) return null;
  const primary = m.metrics.find((v) => v.primary);
  const quality = qualityMetric(m.metrics);
  const label = primary?.label ?? 'Activity';
  const daily = progress(m.today, m.daily_target);

  return (
    <Link
      to={`/w/${ws}/performance`}
     
      aria-label={`Your performance. ${label} today ${ofTarget(m.today, m.daily_target)}, this week ${ofTarget(m.week, m.weekly_target)}${
        quality ? `, ${quality.label} ${formatMetric(quality.since_joining, quality.unit)}` : ''
      }`}
      className={cn(
        'fixed right-36 bottom-4 z-30 hidden items-center gap-3 rounded-sm border border-rule-strong bg-raised px-3 py-1.5 text-[13px] text-ink md:flex',
        'shadow-[0_8px_24px_-12px_rgba(28,27,24,0.5)] hover:border-ink focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-accent',
      )}
    >
      <span className="flex flex-col leading-tight">
        <span className="kicker">{label}</span>
        <span>
          Today <strong>{ofTarget(m.today, m.daily_target)}</strong> · Week <strong>{ofTarget(m.week, m.weekly_target)}</strong>
          {quality && (
            <>
              {' '}
              · {quality.label} <strong>{formatMetric(quality.since_joining, quality.unit)}</strong>
            </>
          )}
        </span>
      </span>
      {m.daily_target ? (
        <span aria-hidden="true" className="h-1.5 w-12 overflow-hidden rounded-xs bg-rule">
          <span className="block h-full bg-accent" style={{ width: `${daily}%` }} />
        </span>
      ) : null}
    </Link>
  );
}
