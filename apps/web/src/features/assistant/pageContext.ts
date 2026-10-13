import type { PageContext } from './types';

export type Viewing = { context: PageContext; label: string | undefined };

/** What the person is looking at, from the URL alone. The server checks every id before using it. */
export function readPageContext(pathname: string, search: string): Viewing {
  const context: PageContext = { route: pathname };
  const parts = pathname.split('/').filter(Boolean);
  const [root, , page, id] = parts;
  if (root !== 'w') return { context, label: undefined };
  if (page === 'inbox' && id) return { context: { ...context, message_id: id }, label: 'this email' };
  if (page === 'agents' && id) return { context: { ...context, case_id: id }, label: 'this agent case' };
  if (page === 'matching') {
    const req = new URLSearchParams(search).get('req');
    if (req) return { context: { ...context, requirement_id: req }, label: 'this requirement' };
  }
  return { context, label: undefined };
}
