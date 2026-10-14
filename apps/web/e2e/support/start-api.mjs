// Starts the real API (+ worker) for browser E2E: a fresh SQLite file, AI_MOCK=1, captured mail, and the
// built web app served from the same origin. Builds the binary first unless E2E_API_BIN points at one.
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const webDir = path.resolve(here, '../..');
const apiDir = path.resolve(webDir, '../api');
const port = Number(process.env.E2E_API_PORT ?? 18080);
const nangoPort = Number(process.env.E2E_NANGO_PORT ?? 18081);
const runDir = process.env.E2E_RUN_DIR ?? path.join(os.tmpdir(), 'post-room-e2e');
const profile = process.env.E2E_API_PROFILE === 'release' ? 'release' : 'debug';

rmSync(runDir, { recursive: true, force: true });
mkdirSync(path.join(runDir, 'auth'), { recursive: true });

let bin = process.env.E2E_API_BIN;
if (!bin) {
  const args = ['build', '--manifest-path', path.join(apiDir, 'Cargo.toml'), '--bin', 'email-ai-api', ...(profile === 'release' ? ['--release'] : [])];
  const built = spawnSync(process.env.CARGO ?? 'cargo', args, { stdio: 'inherit' });
  if (built.status !== 0) process.exit(built.status ?? 1);
  bin = path.join(apiDir, 'target', profile, 'email-ai-api');
}

const dist = path.join(webDir, 'dist');
if (!existsSync(path.join(dist, 'index.html'))) {
  console.error(`No built web app at ${dist}. Run "pnpm build" (or "vite build") first.`);
  process.exit(1);
}

const origin = `http://127.0.0.1:${port}`;
const child = spawn(bin, ['serve'], {
  cwd: runDir,
  stdio: 'inherit',
  env: {
    PATH: process.env.PATH,
    HOME: process.env.HOME,
    RUST_LOG: process.env.RUST_LOG ?? 'warn,email_ai_api=info',
    APP_ENV: 'e2e',
    AI_MOCK: '1',
    MAIL_CAPTURE_FILE: path.join(runDir, 'mail.jsonl'),
    DATABASE_URL: `sqlite://${path.join(runDir, 'e2e.db')}`,
    API_BIND_ADDR: `127.0.0.1:${port}`,
    PUBLIC_WEB_URL: origin,
    APP_BASE_URL: origin,
    WEB_DIST_DIR: dist,
    AUTH_MODE: 'password',
    SIGNUP_ENABLED: 'true',
    SIGNUP_ALLOWED_DOMAINS: 'e2e.test',
    ALLOW_TEST_EMAILS: 'true',
    NANGO_API_KEY: 'e2e-nango-key',
    NANGO_API_URL: `http://127.0.0.1:${nangoPort}`,
    NANGO_WEBHOOK_SECRET: 'e2e-nango-webhook-secret',
    NANGO_BACKFILL_DAYS: '30',
    AI_AUTO_HOURS: '720',
    AUTO_DRAFTS: 'false',
    SEND_UNDO_SECONDS: process.env.E2E_SEND_UNDO_SECONDS ?? '10',
    JOB_WORKER_CONCURRENCY: '4',
    INBOUND_DOMAIN: 'inbound.e2e.test',
  },
});

const stop = () => child.kill('SIGTERM');
process.on('SIGTERM', stop);
process.on('SIGINT', stop);
child.on('exit', (code) => process.exit(code ?? 0));
