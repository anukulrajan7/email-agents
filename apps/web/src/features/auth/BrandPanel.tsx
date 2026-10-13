import type { CSSProperties } from 'react';
import { Stamp } from '../../components/ui';
import { cn } from '../../lib/cn';

type Letter = {
  from: string;
  subject: string;
  stamp: string;
  color: string;
  tilt: number;
  position: string;
};

const LETTERS: Letter[] = [
  { from: 'arjun.m@acme.in', subject: 'Leave request, 12–14 June', stamp: 'Leave', color: 'ochre', tilt: 4, position: 'top-0 left-14' },
  { from: 'referrals@talentco.com', subject: 'Referral: Meera S., React, 5 yrs', stamp: 'Candidate', color: 'sage', tilt: -3, position: 'top-[7.5rem] left-0' },
  {
    from: 'hiring@northwind.io',
    subject: 'Need 3 Java developers in Pune',
    stamp: 'Client requirement',
    color: 'postmark',
    tilt: 1.5,
    position: 'top-[15rem] left-8',
  },
];

const vars = (i: number, tilt: number) =>
  ({ '--i': i, '--tilt': `${tilt}deg`, '--tilt-from': `${tilt * 2.5}deg`, transform: `rotate(${tilt}deg)` }) as CSSProperties;

function PostageStamp() {
  return (
    <span
      aria-hidden="true"
      className="flex h-14 w-12 shrink-0 flex-col items-center justify-center border-[3px] border-dotted border-raised bg-accent-tint outline outline-1 outline-rule-strong"
    >
      <span className="type-display text-xl leading-none text-accent">P</span>
      <span className="mt-0.5 font-mono text-[8px] tracking-[0.12em] text-accent">1¢</span>
    </span>
  );
}

function LetterCard({ letter, index, front }: { letter: Letter; index: number; front: boolean }) {
  return (
    <div className={cn('letter letter-drop absolute w-[22rem] rounded-xs', letter.position)} style={vars(index, letter.tilt)}>
      <div aria-hidden="true" className="airmail h-1" />
      <div className="flex gap-4 px-5 pt-4 pb-5">
        <div className="min-w-0 flex-1">
          <p className="truncate font-mono text-[11px] text-ink-muted">{letter.from}</p>
          <p className="type-display mt-1 text-lg leading-snug text-ink">{letter.subject}</p>
          <div aria-hidden="true" className="mt-3 flex flex-col gap-1.5">
            <span className="block h-[3px] w-48 rounded-full bg-rule" />
            <span className="block h-[3px] w-36 rounded-full bg-rule" />
          </div>
        </div>
        <div className="relative">
          <PostageStamp />
          {front && (
            <span
              aria-hidden="true"
              className="postmark-ring absolute -bottom-6 -left-9 flex h-14 w-14 rotate-[-12deg] items-center justify-center font-mono text-[8px] font-semibold tracking-[0.14em] text-ink/50"
            >
              SORTED
            </span>
          )}
        </div>
      </div>
      <span className="stamp-press absolute -bottom-3 right-5 inline-block" style={vars(index, -3)}>
        <Stamp name={letter.stamp} color={letter.color} className="bg-raised" />
      </span>
    </div>
  );
}

export function BrandPanel() {
  return (
    <section
      aria-label="About Post Room"
      className="relative flex flex-col overflow-hidden border-b border-rule-strong bg-surface px-4 py-6 sm:px-8 lg:border-r lg:border-b-0 lg:px-14 lg:py-14"
    >
      <div className="rise" style={{ '--i': 0 } as CSSProperties}>
        <p className="kicker hidden lg:block">The sorting office for HR mail</p>
        <p className="type-display text-[2.5rem] leading-none tracking-[-0.035em] text-ink sm:text-5xl lg:mt-4 lg:text-[5.5rem]">
          Post <span className="text-accent">Room</span>
        </p>
        <p className="mt-3 max-w-[34ch] text-[15px] leading-relaxed text-ink-muted lg:mt-6 lg:text-lg">
          Every email to your recruiting and HR desk, read, sorted and drafted. You approve anything that goes out.
        </p>
        <ul aria-label="Sorted into" className="mt-4 flex flex-wrap gap-2 lg:hidden">
          {LETTERS.slice()
            .reverse()
            .map((l) => (
              <li key={l.stamp}>
                <Stamp name={l.stamp} color={l.color} />
              </li>
            ))}
        </ul>
      </div>
      <div aria-hidden="true" className="relative mt-14 hidden h-[24rem] w-[26rem] max-w-full lg:block xl:ml-6">
        {LETTERS.map((l, i) => (
          <LetterCard key={l.stamp} letter={l} index={i} front={i === LETTERS.length - 1} />
        ))}
      </div>
      <p className="mt-auto hidden pt-10 font-mono text-[11px] tracking-[0.08em] text-ink-muted uppercase lg:block">
        Read by AI · Sorted by your labels · Sent only with your OK
      </p>
    </section>
  );
}
