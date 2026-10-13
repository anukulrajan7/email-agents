export function PlainTextBody({ text }: { text: string }) {
  return <pre className="font-sans text-[15px] leading-relaxed whitespace-pre-wrap break-words text-ink">{text}</pre>;
}
