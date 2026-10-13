const rtf = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
const numberFormat = new Intl.NumberFormat('en');

export function relativeTime(iso: string, now = Date.now()): string {
  const then = new Date(iso);
  const diffSec = Math.round((then.getTime() - now) / 1000);
  const abs = Math.abs(diffSec);
  if (Number.isNaN(diffSec)) return '';
  if (abs < 45) return 'just now';
  if (abs < 3600) return rtf.format(Math.round(diffSec / 60), 'minute');
  if (abs < 86400) return rtf.format(Math.round(diffSec / 3600), 'hour');
  if (abs < 7 * 86400) return rtf.format(Math.round(diffSec / 86400), 'day');
  const sameYear = then.getFullYear() === new Date(now).getFullYear();
  return then.toLocaleDateString('en-GB', { day: 'numeric', month: 'short', ...(sameYear ? {} : { year: 'numeric' }) });
}

export const fullDate = (iso: string) =>
  new Date(iso).toLocaleString('en-GB', { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' });

export const formatNumber = (n: number) => numberFormat.format(n);

export const formatPercent = (share: number) => `${Math.round(share * 100)}%`;

export function formatCost(usd: number): string {
  const cents = usd * 100;
  if (cents === 0) return '0¢';
  if (cents < 0.05) return '<0.1¢';
  if (cents >= 100) return `$${(usd).toFixed(2)}`;
  return `${Number(cents.toFixed(1))}¢`;
}

export const plural = (n: number, one: string, many = `${one}s`) => `${formatNumber(n)} ${n === 1 ? one : many}`;

export function humanize(value: string): string {
  const s = value.replace(/_/g, ' ');
  return s.charAt(0).toUpperCase() + s.slice(1);
}

export function formatBytes(n: number | null): string {
  if (n === null) return 'unknown size';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function postmarkParts(iso: string): { date: string; time: string } {
  const d = new Date(iso);
  const date = d
    .toLocaleDateString('en-GB', { day: '2-digit', month: 'short', year: 'numeric' })
    .toUpperCase();
  const time = d.toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', hour12: false });
  return { date, time };
}
