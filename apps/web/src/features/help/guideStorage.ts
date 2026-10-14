const key = (user: string, ws: string) => `guide:v2:${user}:${ws}:dismissed`;

export function readDismissed(user: string, ws: string): Set<string> {
  try {
    const parsed: unknown = JSON.parse(localStorage.getItem(key(user, ws)) ?? '[]');
    return new Set(Array.isArray(parsed) ? parsed.filter((v): v is string => typeof v === 'string') : []);
  } catch {
    return new Set();
  }
}

export const writeDismissed = (user: string, ws: string, ids: Set<string>) => localStorage.setItem(key(user, ws), JSON.stringify([...ids]));
