import { postmarkParts } from '../../lib/format';

export function Postmark({ receivedAt }: { receivedAt: string }) {
  const { date, time } = postmarkParts(receivedAt);
  return (
    <svg
      role="img"
      aria-label={`Received ${date} ${time}`}
      viewBox="0 0 112 112"
      className="h-28 w-28 shrink-0 rotate-[-9deg] text-ink-muted"
      fill="none"
      stroke="currentColor"
    >
      <circle cx="56" cy="56" r="53" strokeWidth="1.5" strokeDasharray="3 3" />
      <circle cx="56" cy="56" r="44" strokeWidth="1" />
      <path d="M18 44h76M18 68h76" strokeWidth="1" />
      <g fill="currentColor" stroke="none" fontFamily="'JetBrains Mono Variable', ui-monospace, monospace" textAnchor="middle">
        <text x="56" y="34" fontSize="7" letterSpacing="1.5">
          POST ROOM
        </text>
        <text x="56" y="60" fontSize="9.5" fontWeight="600">
          {date}
        </text>
        <text x="56" y="87" fontSize="9" letterSpacing="1">
          {time}
        </text>
      </g>
    </svg>
  );
}
