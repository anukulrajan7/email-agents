const base = (user: string, ws: string) => `guide:v1:${user}:${ws}`;

function readList(key: string): string[] {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key) ?? '[]');
    return Array.isArray(parsed) ? parsed.filter((v): v is string => typeof v === 'string') : [];
  } catch {
    return [];
  }
}

const writeList = (key: string, items: Iterable<string>) => localStorage.setItem(key, JSON.stringify([...items]));

export const readTried = (user: string, ws: string) => new Set(readList(`${base(user, ws)}:tried`));
export const writeTried = (user: string, ws: string, tried: Set<string>) => writeList(`${base(user, ws)}:tried`, tried);
export const readNudged = (user: string, ws: string) => new Set(readList(`${base(user, ws)}:nudged`));
export const writeNudged = (user: string, ws: string, pages: Set<string>) => writeList(`${base(user, ws)}:nudged`, pages);
