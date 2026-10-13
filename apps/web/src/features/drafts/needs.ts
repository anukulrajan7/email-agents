const NEEDS_SPLIT_RE = /(\[NEEDS:[^\]]*\])/g;
const NEEDS_OPEN_RE = /\[NEEDS:/;

export const hasNeeds = (text: string): boolean => NEEDS_OPEN_RE.test(text);

export function splitNeeds(text: string): { text: string; needs: boolean }[] {
  return text
    .split(NEEDS_SPLIT_RE)
    .filter((part) => part !== '')
    .map((part) => ({ text: part, needs: part.startsWith('[NEEDS:') && part.endsWith(']') }));
}

export function extractNeeds(text: string): string[] {
  return splitNeeds(text)
    .filter((p) => p.needs)
    .map((p) => p.text.slice('[NEEDS:'.length, -1).trim());
}

export function isStillNeeded(text: string, item: string): boolean {
  const wanted = item.trim().toLowerCase();
  return extractNeeds(text).some((n) => {
    const have = n.toLowerCase();
    return have.includes(wanted) || wanted.includes(have);
  });
}
