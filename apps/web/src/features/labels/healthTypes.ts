export type TrendPoint = { date: string; count: number };

export type LabelHealthRow = {
  key: string;
  name: string;
  count: number;
  share: number;
  auto: number;
  corrected_away: number;
  corrected_into: number;
  correction_rate: number;
  avg_confidence: number | null;
  trend: TrendPoint[];
};

export type HealthAdvice = {
  kind: 'add_label' | 'label_unreliable' | 'label_unused';
  label_key: string | null;
  text: string;
};

export type LabelHealthReport = {
  days: number;
  total: number;
  other_share: number;
  auto_share: number;
  labels: LabelHealthRow[];
  other_trend: TrendPoint[];
  advice: HealthAdvice[];
};
