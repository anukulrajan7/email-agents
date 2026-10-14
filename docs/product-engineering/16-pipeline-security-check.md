# 16 — Pipeline and security check (15 Oct 2026)

**Summary.** A read-only check of the CI/CD pipelines, the Docker build, the repository contents and the Rust job pipeline for anything suspicious. No sign of compromise or leaked secrets. Two findings need action soon: **production deploys even when the browser e2e tests fail**, and **CI holds a root SSH key to the production server while using third-party actions by mutable tag**. Everything else is hygiene.

## Findings

| # | Severity | Finding | Evidence | Fix |
|---|---|---|---|---|
| P1 | High | Deploys ship even when e2e fails. e2e failed on 3 of the last 5 pushes to main, and each of those commits was deployed. | e2e runs 38594535089, 38545147805, 38528776253 failed; `deploy.yml` does not depend on e2e. Failure: `e2e/connect-forwarding.spec.ts:66` still expects "Emails already here stay in Post Room." after the purge flow changed. | Update the spec text; make e2e a required check before deploy (add it to `deploy.yml` or branch protection). |
| P2 | High | CI gets root on the production server, in jobs that use actions by mutable tag. | `deploy.yml` and `daily-selftest.yml` SSH as `root@${VPS_HOST}` with `VPS_SSH_KEY`; `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `docker/*@v3/v6` are tags; `deploy.yml` has a `TODO(security): pin third-party actions` | Pin every action to a commit SHA (some in `daily-selftest.yml` already are); deploy as a non-root user in the `docker` group; scope the SSH key to that user. |
| P3 | Medium | Compiled Python bytecode is committed. | `scripts/test-pack/__pycache__/check.cpython-314.pyc`, `scenarios.cpython-314.pyc` (merged with `wp-testpack`) | Delete them; add `__pycache__/` to `.gitignore`. Bytecode cannot be reviewed. |
| P4 | Medium | Test email injection is on in production. | `deploy.yml` writes `ALLOW_TEST_EMAILS=true`; `.env.example` says "Local demo only" | It is admin-only, rate-limited to 20 a day, forwarding-only, and forwarding sources cannot send, so it cannot trigger an outbound email. But the sender is free text and the mail runs the paid AI pipeline and creates contacts and memories. Decide on purpose; if it is the onboarding demo, keep it and document why. |
| P5 | Medium | Two sessions changed main at the same time. | Merges at 05:00 and 05:07 on 15 Oct landed while a build was running; that build saw half-merged files (`expected ... found SendMode`). Same author, normal merges; no tampering. | The one-agent, no-worktree rule (from 15 Oct) prevents this. Merge only from a clean, idle main. |
| P6 | Medium | Rust CI time swings 11 to 33 minutes. | Runs 38545148191 and 38594535204 | Test speed plan (`15-test-speed-plan.md`). |
| P7 | Low | Base images and tools are unpinned. | `rust:1-bookworm`, `node:24-bookworm-slim`, `debian:bookworm-slim`; `npm install -g pnpm@12.5.1` | Pin image digests for reproducible builds. |
| P8 | Low | `wp-evals` and `verify-agentfix` worktrees still hold uncommitted changes. | 7 and 1 modified files | Decide: commit or discard, then remove the worktrees. |
| P9 | Info | Production runs `JOB_WORKER_CONCURRENCY=4`, not 2. | `deploy.yml` | Capacity is about 23,000 emails a day, double the figure in tab 06. |

## Checked and clean

- No secrets in tracked files or anywhere in git history. Patterns checked: OpenRouter keys, GitHub tokens, private keys, AWS keys, Slack tokens, webhook secrets.
- `.env` is git-ignored and was never committed.
- No `build.rs` (no code runs at build time from this repo).
- Docker runs the app as a non-root user (uid 10001); the API binds to `127.0.0.1:18090` behind Caddy.
- Deploy writes secrets to a file with `umask 077`, copies it, and deletes it; the registry login is logged out after the pull.
- Webhooks are signature-checked (Svix for Resend with a 5-minute window; HMAC for Nango, no time window, see tab 07 H12).

## Rust job pipeline (from the audit, still open)

No lease heartbeat (jobs over 120 s run twice); panics inside jobs are not recorded; deferred AI jobs are marked `succeeded`; budget holes (drafts, assistant, Jev); `injection_signals` never used. Details and fixes are in `07-engineering-mistakes-and-improvements.md`.
