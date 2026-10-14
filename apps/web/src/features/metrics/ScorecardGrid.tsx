import { Stat } from './Stat';
import { distance, money, millis, percent } from './format';
import type { Totals } from './types';

export function ScorecardGrid({ totals, days }: { totals: Totals; days: number }) {
  return (
    <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
      <Stat label="Cases" value={String(totals.cases)} note={`Last ${days} days`} />
      <Stat label="Completed" value={percent(totals.success_rate)} note="Cases that finished without failing" />
      <Stat label="Approved" value={percent(totals.approval_rate)} note={`${totals.approvals_decided} decisions by people`} />
      <Stat label="Declined" value={percent(totals.decline_rate)} note="Drafts people turned down" />
      <Stat label="Edits before sending" value={distance(totals.edit_distance_median)} note="0 is unchanged, 1 is rewritten" />
      <Stat label="Stopped" value={percent(totals.stop_rate)} note={`${totals.auto_sends} automatic sends`} />
      <Stat label="Cost per case" value={money(totals.cost_per_case_usd)} note={`${money(totals.cost_per_email_usd)} per email`} />
      <Stat label="Speed" value={millis(totals.latency_p50_ms)} note={`Slowest 5%: ${millis(totals.latency_p95_ms)}`} />
    </div>
  );
}
