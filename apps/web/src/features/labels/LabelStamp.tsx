import { Stamp } from '../../components/ui';
import { useLabelMap } from './hooks';

export function LabelStamp({ ws, category, size, tilted }: { ws: string; category: string; size?: 'sm' | 'lg'; tilted?: boolean }) {
  const label = useLabelMap(ws).get(category);
  return label ? <Stamp name={label.name} color={label.color} size={size} tilted={tilted} /> : <Stamp name={category} size={size} tilted={tilted} />;
}
