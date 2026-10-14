import { Card } from '../../components/ui';

export function Stat({ label, value, note }: { label: string; value: string; note?: string }) {
  return (
    <Card>
      <p className="kicker">{label}</p>
      <p className="type-display mt-1 text-3xl tracking-[-0.02em] text-ink">{value}</p>
      {note && <p className="mt-1 text-sm text-ink-muted">{note}</p>}
    </Card>
  );
}
