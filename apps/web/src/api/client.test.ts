import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { api, ApiError, buildUrl, TOKEN_KEY } from './client';

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });

describe('api client', () => {
  beforeEach(() => localStorage.clear());
  afterEach(() => vi.unstubAllGlobals());

  it('adds the bearer token and parses success bodies', async () => {
    localStorage.setItem(TOKEN_KEY, 'dev:a@b.test');
    const fetchMock = vi.fn().mockResolvedValue(json(200, { items: [] }));
    vi.stubGlobal('fetch', fetchMock);

    await expect(api('/workspaces')).resolves.toEqual({ items: [] });
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit];
    expect(url).toBe('/api/v1/workspaces');
    expect((init.headers as Record<string, string>)['X-Auth-Token']).toBe('dev:a@b.test');
  });

  it('parses the documented error shape into ApiError', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(json(403, { error: { code: 'forbidden', message: 'Admins only', request_id: 'req_1' } })),
    );
    const err = await api('/x').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect(err).toMatchObject({ status: 403, code: 'forbidden', message: 'Admins only', requestId: 'req_1' });
  });

  it('clears the token and redirects to /sign-in on 401', async () => {
    localStorage.setItem(TOKEN_KEY, 'dev:a@b.test');
    const assign = vi.fn();
    vi.stubGlobal('location', { pathname: '/workspaces', assign });
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(json(401, { error: { code: 'unauthorized', message: 'nope', request_id: 'r' } })),
    );
    await expect(api('/me')).rejects.toBeInstanceOf(ApiError);
    expect(localStorage.getItem(TOKEN_KEY)).toBeNull();
    expect(assign).toHaveBeenCalledWith('/sign-in');
  });

  it('builds query strings and skips empty values', () => {
    expect(buildUrl('/m', { cursor: undefined, q: '', category: 'leave', limit: 25 })).toBe('/api/v1/m?category=leave&limit=25');
  });
});
