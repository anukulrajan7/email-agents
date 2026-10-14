import { Link } from 'react-router';
import { Badge, Card, EmptyState, Page, PageHeader, Spinner, Table, Td, Th } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { AgentAvatar } from '../agents/AgentAvatar';
import { ScorecardGrid } from './ScorecardGrid';
import { LEVEL_NAMES, money, percent } from './format';
import { useScorecard } from './hooks';

export function MetricsRoute() {
  const { ws } = useWorkspace();
  const scorecard = useScorecard(ws);
  const data = scorecard.data;

  return (
    <Page className="gap-6">
      <PageHeader
        kicker="Results"
        title="How your agents are doing"
        description="Cases, approvals, edits, cost and speed for each agent, and how much freedom each one has earned."
      />
      {scorecard.isLoading && <Spinner />}
      <ErrorNote error={scorecard.error} />
      {data && (
        <>
          <div className="rise" style={riseStyle(1)}>
            <ScorecardGrid totals={data.totals} days={data.days} />
          </div>
          <section className="rise flex flex-col gap-3" style={riseStyle(2)} aria-label="Agents">
            {data.agents.length === 0 ? (
              <Card>
                <EmptyState title="No agents yet" description="Once an agent handles emails, its results show up here." />
              </Card>
            ) : (
              <Table>
                <caption className="sr-only">Results for each agent over the last {data.days} days</caption>
                <thead>
                  <tr>
                    <Th>Agent</Th>
                    <Th className="text-right">Cases</Th>
                    <Th className="text-right">Approved</Th>
                    <Th className="text-right">Cost per email</Th>
                    <Th>Level</Th>
                    <Th>Safety checks</Th>
                  </tr>
                </thead>
                <tbody>
                  {data.agents.map(({ agent, totals, autonomy, safety }) => (
                    <tr key={agent.id}>
                      <Td>
                        <Link to={`/w/${ws}/metrics/${agent.id}`} className="inline-flex items-center gap-2 font-semibold text-ink underline-offset-4 hover:underline">
                          <AgentAvatar agent={agent} />
                          {agent.name}
                        </Link>
                      </Td>
                      <Td className="text-right font-mono text-xs">{totals.cases}</Td>
                      <Td className="text-right font-mono text-xs">{percent(totals.approval_rate)}</Td>
                      <Td className="text-right font-mono text-xs">{money(totals.cost_per_email_usd)}</Td>
                      <Td>
                        {LEVEL_NAMES[autonomy.earned] ?? autonomy.earned}
                        {autonomy.ready_for !== null && (
                          <>
                            {' '}
                            <Badge tone="success">Ready for level {autonomy.ready_for}</Badge>
                          </>
                        )}
                      </Td>
                      <Td>
                        {safety ? (
                          <Badge tone={safety.status === 'passed' ? 'success' : 'warning'}>{safety.status === 'passed' ? 'Passing' : 'Needs attention'}</Badge>
                        ) : (
                          <span className="text-ink-muted">Not run yet</span>
                        )}
                      </Td>
                    </tr>
                  ))}
                </tbody>
              </Table>
            )}
          </section>
        </>
      )}
    </Page>
  );
}
