import { Stamp } from '../../components/ui';
import { useLabelMap } from './hooks';

export function LabelStamp({ ws, category, size, tilted }: { ws: string; category: string; size?: 'sm' | 'lg'; tilted?: boolean }) {
  const label = useLabelMap(ws).get(category);
  if (!label) return <Stamp name={category} size={size} tilted={tilted} />;
  const stamp = <Stamp name={label.name} color={label.color} size={size} tilted={tilted} />;
  return label.archived ? (
    <span className="opacity-50" title="This label is archived">
      {stamp}
    </span>
  ) : (
    stamp
  );
}
