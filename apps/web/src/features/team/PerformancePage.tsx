import { Link } from 'react-router';
import { Button, Card, CardTitle, EmptyState, Page, PageHeader, SkeletonList, Table, Td, Th } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { SubmissionsPanel } from '../hrms/SubmissionsPanel';
import { formatMetric, ofTarget, progress, qualityMetric } from './format';
import { useMyMetrics, useTeamAccess, useTeamMetrics } from './hooks';
import type { PersonMetrics } from './types';

function Bar({ value, target }: { value: number; target: number | null }) {
  if (!target) return null;
  return (
    <span aria-hidden="true" className="mt-1 block h-1 w-20 overflow-hidden rounded-xs bg-rule">
      <span className="block h-full bg-accent" style={{ width: `${progress(value, target)}%` }} />
    </span>
  );
}

function Row({ p }: { p: PersonMetrics }) {
  const primary = p.metrics.find((m) => m.primary);
  const quality = qualityMetric(p.metrics);
  return (
    <tr>
      <Td>
        <p className="font-semibold text-ink">{p.name}</p>
        {p.role && <p className="text-xs text-ink-muted">{p.role}</p>}
      </Td>
      <Td>
        {ofTarget(p.today, p.daily_target)}
        <Bar value={p.today} target={p.daily_target} />
      </Td>
      <Td>
        {ofTarget(p.week, p.weekly_target)}
        <Bar value={p.week} target={p.weekly_target} />
      </Td>
      <Td>{formatMetric(primary?.since_joining, primary?.unit ?? 'count')}</Td>
      <Td>{quality ? formatMetric(quality.since_joining, quality.unit) : '–'}</Td>
    </tr>
  );
}

export function PerformancePage() {
  const { ws } = useWorkspace();
  const access = useTeamAccess(ws);
  const mine = useMyMetrics(ws);
  const team = useTeamMetrics(ws);
  const staffing = access.data?.staffing ?? false;
  const isOwner = access.data?.level === 'super_admin';
  const members = team.data?.members ?? [];
  const primaryLabel = team.data?.metrics.find((m) => m.primary)?.label ?? 'Activity';
  const qualityLabel = team.data ? qualityMetric(team.data.metrics)?.label : undefined;
  const me = mine.data?.metrics;

  return (
    <Page className="gap-8">
      <PageHeader
        guide="performance-header"
        kicker={staffing ? 'HRMS · Performance' : 'Team · Performance'}
        title="Team performance"
        description="Today, this week and since joining, measured against each person's target. Every number is counted from your mail, never estimated by AI."
      />
      <ErrorNote error={team.error ?? mine.error} />
      {(team.isLoading || mine.isLoading) && <SkeletonList rows={3} label="Loading performance" />}

      {me && (
        <section aria-label="Your numbers" data-guide="performance-numbers" className="rise flex flex-col gap-3" style={riseStyle(1)}>
          <CardTitle>Your numbers</CardTitle>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            <Card>
              <p className="kicker">{primaryLabel} today</p>
              <p className="type-display text-3xl">{ofTarget(me.today, me.daily_target)}</p>
            </Card>
            <Card>
              <p className="kicker">{primaryLabel} this week</p>
              <p className="type-display text-3xl">{ofTarget(me.week, me.weekly_target)}</p>
            </Card>
            {me.metrics
              .filter((m) => !m.primary && (m.unit !== 'count' || m.key === 'interviews'))
              .map((m) => (
                <Card key={m.key}>
                  <p className="kicker">{m.label}</p>
                  <p className="type-display text-3xl">{formatMetric(m.since_joining, m.unit)}</p>
                  <p className="text-xs text-ink-muted">since joining</p>
                </Card>
              ))}
          </div>
        </section>
      )}

      {team.data && (
        <section aria-label="Team" className="rise flex flex-col gap-3" style={riseStyle(2)}>
          <CardTitle>{isOwner ? 'Everyone' : 'Your team'}</CardTitle>
          {members.length === 0 ? (
            <Card>
              {isOwner ? (
                <EmptyState
                  title="No one to measure yet"
                  description="Add the people on your team first, with their manager and daily and weekly targets. Their numbers appear here as mail comes in."
                  action={
                    <Link to={`/w/${ws}/people`}>
                      <Button>Go to People</Button>
                    </Link>
                  }
                />
              ) : (
                <EmptyState
                  title="No team to show"
                  description="This page lists the people who report to you in the manager chain. Your own numbers are above."
                />
              )}
            </Card>
          ) : (
            <Table>
              <thead>
                <tr>
                  <Th>Person</Th>
                  <Th>{primaryLabel} today</Th>
                  <Th>This week</Th>
                  <Th>Since joining</Th>
                  <Th>{qualityLabel ?? 'Quality'}</Th>
                </tr>
              </thead>
              <tbody>
                {members.map((p) => (
                  <Row key={p.person_id} p={p} />
                ))}
              </tbody>
            </Table>
          )}
        </section>
      )}

      {staffing && <SubmissionsPanel ws={ws} />}
    </Page>
  );
}
