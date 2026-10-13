import { render, screen } from '@testing-library/react';
import type { Insight } from '../../api/types';
import { InsightCard } from './InsightCard';

const insight: Insight = {
  summary: 'Vendor wants the September invoice approved.',
  asks: ['Approve invoice #123', 'Confirm payment date'],
  deadline: 'Friday 5pm',
  needs_reply: true,
  missing_info: ['PO number'],
};

describe('InsightCard', () => {
  it('shows summary, asks, deadline, reply chip and missing info', () => {
    render(<InsightCard insight={insight} status="processed" />);
    expect(screen.getByText("What they're asking")).toBeInTheDocument();
    expect(screen.getByText('Vendor wants the September invoice approved.')).toBeInTheDocument();
    expect(screen.getAllByRole('listitem').map((li) => li.textContent)).toEqual(['Approve invoice #123', 'Confirm payment date', 'PO number']);
    expect(screen.getByText('Deadline: Friday 5pm')).toBeInTheDocument();
    expect(screen.getByText('Needs reply')).toBeInTheDocument();
    expect(screen.getByText('PO number')).toBeInTheDocument();
  });

  it('says when no reply is needed and omits empty sections', () => {
    render(<InsightCard insight={{ ...insight, needs_reply: false, deadline: null, asks: [], missing_info: [] }} status="processed" />);
    expect(screen.getByText('No reply needed')).toBeInTheDocument();
    expect(screen.queryByText('Needs reply')).toBeNull();
    expect(screen.queryByText(/Deadline/)).toBeNull();
    expect(screen.queryByRole('list')).toBeNull();
  });

  it('shows a quiet skeleton while the insight is still null', () => {
    render(<InsightCard insight={null} status="classified" />);
    expect(screen.getByText('Reading this email…')).toBeInTheDocument();
  });

  it('renders nothing when the API sends no insight field or processing failed', () => {
    const { container, rerender } = render(<InsightCard insight={undefined} status="processed" />);
    expect(container).toBeEmptyDOMElement();
    rerender(<InsightCard insight={null} status="failed" />);
    expect(container).toBeEmptyDOMElement();
  });
});
