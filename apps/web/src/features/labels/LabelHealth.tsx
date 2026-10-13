import { useState } from 'react';
import { Badge, Button, Card, EmptyState, Mono, SegmentedControl, SkeletonTable, Stamp, Table, Td, Th, type Tone } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useLabelHealth, useLabelMap } from './hooks';
import type { HealthAdvice, LabelHealthRow, TrendPoint } from './healthTypes';

const RANGES = [7, 30, 90] as const;
const MIN_EMAILS = 5;

export type HealthAction = { kind: 'suggest' } | { kind: 'edit'; key: string };

const percent = (v: number) => `${Math.round(v * 100)}%`;

export type Reliability = { text: 'Reliable' | 'Watch' | 'Unreliable' | 'No data'; tone: Tone };

export function reliability(row: Pick<LabelHealthRow, 'auto' | 'corrected_away' | 'correction_rate'>): Reliability {
  const picks = row.auto + row.corrected_away;
  if (picks === 0) return { text: 'No data', tone: 'neutral' };
  if (row.correction_rate >= 0.2 && picks >= MIN_EMAILS) return { text: 'Unreliable', tone: 'danger' };
  if (row.correction_rate >= 0.1) return { text: 'Watch', tone: 'warning' };
  return { text: 'Reliable', tone: 'success' };
}

export function Sparkline({ trend, label }: { trend: TrendPoint[]; label: string }) {
  const width = 80;
  const height = 20;
  const max = Math.max(1, ...trend.map((p) => p.count));
  const step = trend.length > 1 ? width / (trend.length - 1) : 0;
  const points = trend.map((p, i) => `${(i * step).toFixed(1)},${(height - 2 - (p.count / max) * (height - 4)).toFixed(1)}`).join(' ');
  return (
    <svg role="img" aria-label={label} width={width} height={height} viewBox={`0 0 ${width} ${height}`} className="text-ink-muted">
      <polyline points={points} fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round" strokeLinecap="round" />
    </svg>
  );
}

function ShareBar({ share }: { share: number }) {
  return (
    <div className="flex items-center gap-2">
      <div aria-hidden="true" className="h-2 w-24 overflow-hidden rounded-xs bg-rule">
        <div className="h-full bg-ink-muted" style={{ width: `${Math.min(100, Math.round(share * 100))}%` }} />
      </div>
      <Mono>{percent(share)}</Mono>
    </div>
  );
}

const sum = (trend: TrendPoint[]) => trend.reduce((n, p) => n + p.count, 0);

function HealthRow({
  name,
  color,
  count,
  share,
  trend,
  days,
  rate,
}: {
  name: string;
  color: string | undefined;
  count: number;
  share: number;
  trend: TrendPoint[];
  days: number;
  rate: { row: LabelHealthRow; verdict: Reliability } | undefined;
}) {
  return (
    <tr>
      <Td>
        <Stamp name={name} color={color} />
      </Td>
      <Td>
        <Mono>{count}</Mono>
      </Td>
      <Td>
        <ShareBar share={share} />
      </Td>
      <Td>
        {rate ? (
          <div className="flex flex-wrap items-center gap-2">
            <Mono>{percent(rate.row.correction_rate)}</Mono>
            <Badge tone={rate.verdict.tone}>{rate.verdict.text}</Badge>
          </div>
        ) : (
          <Mono>—</Mono>
        )}
      </Td>
      <Td>
        <Sparkline trend={trend} label={`${name}: ${count} ${count === 1 ? 'email' : 'emails'} in the last ${days} days`} />
      </Td>
    </tr>
  );
}

function AdviceItem({ advice, isAdmin, onAction }: { advice: HealthAdvice; isAdmin: boolean; onAction: (a: HealthAction) => void }) {
  const key = advice.label_key;
  return (
    <li className="flex flex-wrap items-center justify-between gap-3 rounded-sm border-l-[3px] border-review bg-review-tint px-3 py-2 text-sm text-review-ink">
      <span>{advice.text}</span>
      {isAdmin && advice.kind === 'add_label' && (
        <Button size="sm" variant="secondary" onClick={() => onAction({ kind: 'suggest' })}>
          Suggest labels
        </Button>
      )}
      {isAdmin && advice.kind === 'label_unreliable' && key && (
        <Button size="sm" variant="secondary" onClick={() => onAction({ kind: 'edit', key })}>
          Edit label
        </Button>
      )}
    </li>
  );
}

export function LabelHealth({ ws, isAdmin, onAction }: { ws: string; isAdmin: boolean; onAction: (a: HealthAction) => void }) {
  const [days, setDays] = useState<number>(30);
  const health = useLabelHealth(ws, days);
  const labelMap = useLabelMap(ws);
  const report = health.data;

  return (
    <section aria-labelledby="health-heading" className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 id="health-heading" className="type-display text-2xl tracking-[-0.01em]">
          Label health
        </h2>
        <SegmentedControl label="Time range" options={RANGES.map((d) => ({ value: d as number, label: `${d} days` }))} value={days} onChange={setDays} />
      </div>
      {health.isLoading && <SkeletonTable rows={5} label="Loading label health" />}
      <ErrorNote error={health.error} />
      {report && report.total === 0 && (
        <Card>
          <EmptyState title="No mail to measure yet" description={`No inbound email arrived in the last ${report.days} days. Health appears once mail is labelled.`} />
        </Card>
      )}
      {report && report.total > 0 && (
        <>
          <p role="status" className="text-base text-ink">
            <strong>{percent(report.auto_share)}</strong> of mail is labelled automatically, <strong>{percent(report.other_share)}</strong> lands in Other.
            <span className="ml-2 font-mono text-xs text-ink-muted">
              {report.total} {report.total === 1 ? 'email' : 'emails'} in {report.days} days
            </span>
          </p>
          <Table aria-label="Label health">
            <thead>
              <tr>
                <Th>Label</Th>
                <Th>Emails</Th>
                <Th>Share</Th>
                <Th>People correct it</Th>
                <Th>Trend</Th>
              </tr>
            </thead>
            <tbody>
              {report.labels.map((row) => (
                <HealthRow
                  key={row.key}
                  name={row.name}
                  color={labelMap.get(row.key)?.color}
                  count={row.count}
                  share={row.share}
                  trend={row.trend}
                  days={report.days}
                  rate={{ row, verdict: reliability(row) }}
                />
              ))}
              <HealthRow
                name="Other"
                color={labelMap.get('other')?.color ?? 'rule'}
                count={sum(report.other_trend)}
                share={report.other_share}
                trend={report.other_trend}
                days={report.days}
                rate={undefined}
              />
            </tbody>
          </Table>
          <div className="flex flex-col gap-2">
            <h3 className="kicker">What to do next</h3>
            {report.advice.length === 0 ? (
              <p className="text-sm text-ink-muted">Nothing to fix. Automatic labelling looks healthy.</p>
            ) : (
              <ul aria-label="Advice" className="flex flex-col gap-2">
                {report.advice.map((a) => (
                  <AdviceItem key={`${a.kind}:${a.label_key ?? ''}`} advice={a} isAdmin={isAdmin} onAction={onAction} />
                ))}
              </ul>
            )}
          </div>
        </>
      )}
    </section>
  );
}
