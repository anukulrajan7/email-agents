import { useState } from 'react';
import { Link, useParams } from 'react-router';
import { Page, PageHeader, Spinner, Tabs } from '../../components/ui';
import { ErrorNote } from '../../components/ErrorNote';
import { useWorkspace } from '../../app/useWorkspace';
import { riseStyle } from '../../lib/motion';
import { EvalsPanel } from './EvalsPanel';
import { LevelPanel } from './LevelPanel';
import { ScorecardGrid } from './ScorecardGrid';
import { TrendChart } from './TrendChart';
import { distance, money, percent } from './format';
import { useAgentMetrics } from './hooks';

type Tab = 'scorecard' | 'checks' | 'level';

export function AgentMetricsView() {
  const { agentId = '' } = useParams();
  const { ws, isAdmin } = useWorkspace();
  const metrics = useAgentMetrics(ws, agentId);
  const [tab, setTab] = useState<Tab>('scorecard');
  const data = metrics.data;

  return (
    <Page className="gap-6">
      <PageHeader
        kicker="Results"
        title={data ? data.agent.name : 'Agent results'}
        description="How this agent is doing, the checks it has passed, and why it works at its current level."
        actions={
          <Link to={`/w/${ws}/metrics`} className="text-sm font-semibold text-accent underline underline-offset-4">
            All agents
          </Link>
        }
      />
      {metrics.isLoading && <Spinner />}
      <ErrorNote error={metrics.error} />
      {data && (
        <div className="rise" style={riseStyle(1)}>
          <Tabs
            label="Agent results"
            tabs={[
              { id: 'scorecard', label: 'Scorecard' },
              { id: 'checks', label: 'Checks' },
              { id: 'level', label: 'Why this level' },
            ]}
            value={tab}
            onChange={setTab}
          >
            {tab === 'scorecard' && (
              <>
                <ScorecardGrid totals={data.totals} days={data.days} />
                <div className="grid gap-6 md:grid-cols-3">
                  <TrendChart title="Approved" points={data.series} pick={(p) => p.approval_rate} format={percent} max={1} />
                  <TrendChart title="Edits before sending" points={data.series} pick={(p) => p.edit_distance} format={distance} max={1} />
                  <TrendChart title="Cost per day" points={data.series} pick={(p) => p.cost_usd} format={money} />
                </div>
              </>
            )}
            {tab === 'checks' && <EvalsPanel ws={ws} agentId={agentId} canRun={isAdmin} />}
            {tab === 'level' && <LevelPanel ws={ws} agentId={agentId} canChange={isAdmin} />}
          </Tabs>
        </div>
      )}
    </Page>
  );
}
