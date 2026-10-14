import os from 'node:os';
import path from 'node:path';

export const API_PORT = Number(process.env.E2E_API_PORT ?? 18080);
export const NANGO_PORT = Number(process.env.E2E_NANGO_PORT ?? 18081);

/** Throwaway directory: SQLite file, captured mail, saved sessions. Wiped every run by `start-api.mjs`. */
export const RUN_DIR = process.env.E2E_RUN_DIR ?? path.join(os.tmpdir(), 'post-room-e2e');
export const MAIL_FILE = path.join(RUN_DIR, 'mail.jsonl');
export const SEED_FILE = path.join(RUN_DIR, 'seed.json');
export const AUTH_DIR = path.join(RUN_DIR, 'auth');

export const BASE_URL = process.env.E2E_BASE_URL ?? `http://127.0.0.1:${API_PORT}`;
export const NANGO_URL = `http://127.0.0.1:${NANGO_PORT}`;

export const PASSWORD = 'correct-horse-battery';
export const SIGNUP_DOMAIN = 'e2e.test';

export type Role = 'admin' | 'operator' | 'outsider';
export const authFile = (role: Role) => path.join(AUTH_DIR, `${role}.json`);
