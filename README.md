# AI Email Automation — MVP

Forward an HR email → it shows up in the inbox → it is classified → a timesheet triggers a
**review item + editable acknowledgement draft**. Nothing is ever sent automatically.

- `apps/api` — Rust + Axum + SQLx (SQLite, WAL) + a Tokio worker on a SQLite `jobs` table
- `apps/web` — React + TypeScript + Vite + TanStack Query + Tailwind
- `docs/api.md` — the REST contract both sides follow
- `docs/learn/README.md` — the learning path: Rust, architecture, tools, AI basics, agents, evals, with exercises

## Run locally

Needs Rust (stable) and Node 24 + pnpm.

```bash
cp .env.example .env          # AUTH_MODE=password; for a dev login set AUTH_MODE=dev + ALLOW_DEV_AUTH=1
make api                      # API + worker on :8080 (runs migrations on start)
make web                      # in a second terminal: http://localhost:5173
```

2-minute demo: sign in with any email → create a workspace → **Integrations** → *Set up
forwarding* → **Send test email** → **Inbox**: the message appears, classified as
*Timesheet*, with an open review and a suggested draft → edit / approve / copy the draft →
the audit trail at the bottom shows every step.

`make test` runs the Rust unit + integration tests and the web tests. `make check` runs
clippy (warnings as errors) and the TypeScript typecheck.

## What is real vs. stubbed

| Area | State |
|---|---|
| Workspaces, admin/operator roles, server-side tenant checks | Done |
| Auth | `AUTH_MODE=dev` (`Bearer dev:<email>`) for local use; `AUTH_MODE=oidc` validates RS/ES/PS-signed JWTs against the issuer's JWKS. **Never run dev mode in production.** |
| Forwarding via Resend Inbound | Done: Svix signature check on the raw body, event and job written in one transaction, fetch with `GET /emails/receiving/{id}`, workspace found only from the alias we issued, dedupe on `(source_id, source_message_id)` |
| Test email injection | Runs the same pipeline with `provider='test'`. Gated by `ALLOW_TEST_EMAILS`. |
| Job queue | Atomic claim with `UPDATE … RETURNING`, 120 s leases with `locked_by` fencing, exponential backoff with jitter, dead-letter list and admin retry, sweep of expired leases, at-least-once handlers |
| Classification | Jev adapter (`POST /v1/systemone` with choice / score / yes-no questions) when `TYPESAFE_API_KEY` is set. Otherwise a **rules fallback** stored as `source='fallback'`. ⚠️ The Jev request/response shape comes from a public description and has not been checked against a live key yet. |
| Classification fallback | No Jev key: OpenRouter `OPENROUTER_CLASSIFIER_MODEL_ID` (default `qwen/qwen3.8-flash`) with a strict enum schema, stored as `source='llm'`. |
| Drafts | Two tiers: `OPENROUTER_DRAFT_MODEL_ID` (default `anthropic/claude-haiku-5.5`) for routine replies, `OPENROUTER_PREMIUM_MODEL_ID` (default `OPENROUTER_MODEL_ID`, e.g. `moonshotai/kimi-k2.6`) for high-priority, client or multi-ask mail and "Write a better draft". Both use `response_format: json_schema`, `provider.data_collection: deny`, `reasoning` disabled, and output validation (size, no links). Without a key it uses **template drafts** (`model_id='template-v1'`). |
| Timesheet workflow | Done: one run per (workflow, message), one review per (run, kind), one draft per run |
| Leave / referral workflows | Shown as P1. They can't be enabled yet. |
| Nango Gmail | Done: a short-lived connect session tagged with the workspace and user, then Nango Connect in the browser. The server re-checks the connection's tags with Nango before storing it. A bounded backfill (14 days / 500 messages, newest first), then cursor-based incremental sync from `GET /records`, sharing the same ingest and dedupe as forwarding. Signed webhooks (`auth` creates or degrades, `sync` enqueues), Sync now, and Disconnect. Read only: provider drafts stay off. ⚠️ The record mapping accepts the common Gmail template field names; check it against your Nango sync's actual model. |
| Microsoft 365 | Marked "Not yet supported" until the POC passes |
| Attachments | Metadata only. No download, parsing or scanning (`scan_status='pending'`). |
| Email HTML | Stored, never rendered. The UI shows the plain-text body inside `<pre>`. |

## Agent memory

- **Contacts:** a profile is learned from every email with no LLM: name, count, first and last seen, usual categories (computed from classifications).
- **Memories:** workspace or per-contact notes of kind `fact`, `preference` or `style`, stored in SQLite with an **FTS5** index (`memories_fts`, keyed by memory ID). No vector DB, as the spec asks.
- **Retrieval for drafts:** the sender's notes, then FTS matches on the subject, then recent team notes, within a 2,500-character budget. They go inside `<team_notes>`, kept apart from the untrusted email. Each draft stores `memory_ids_json`, and the UI shows "Used N memories".
- **Learning:** when an operator **approves** a draft they changed, `learn_from_draft` asks the efficient model for at most 3 reusable lessons. They're stored as `status='suggested'` and deduped, and are only used after a person accepts them.
- **Swappable:** this sits behind `memory::context_for` and the `Learner` trait. Mem0, Zep or Letta can replace it later if semantic recall is ever needed.

## Key design choices

- **The webhook does no work.** It verifies the request, writes `inbound_events` and the `fetch_forwarded_email` job in one transaction, and returns 2xx. All provider and AI calls run in the worker, outside any SQLite transaction.
- **Every stage writes its result and enqueues the next job in one transaction.** The first statement in each transaction is a write, so SQLite takes the write lock up front. A read-then-write upgrade could fail with `SQLITE_BUSY` without waiting.
- **Idempotency keys:** `(provider, provider_event_id)`, `(source_id, source_message_id)`, `jobs.dedupe_key`, `(workflow_id, message_id)`, `(workflow_run_id, kind)`, `drafts.origin_key`.
- **`needs_review` is decided by backend policy** (`ai/policy.rs`), never by the model. A manual classification is never overwritten by a later model run.
- **Failures stay visible.** A dead `classify_email` job routes the message to `other` + needs_review. A dead `generate_draft` job marks the run and the message `failed`. Dead jobs appear under **Jobs**, where an admin can retry them.

## Process layout

`email-ai-api serve` runs the API and the worker in one process. For production, run `email-ai-api api` and
`email-ai-api worker` as separate processes **on the same host**. Keep the SQLite file on a local disk,
not a network share. `email-ai-api migrate` only applies migrations.

## Connecting Gmail (Nango)

1. In Nango: create a `google-mail` integration with your Google OAuth app, and enable a Gmail sync that writes the `GmailEmail` model (or set `NANGO_GMAIL_MODEL`).
2. Point Nango's webhook URL at `POST /webhooks/nango`, then copy the signing key into `NANGO_WEBHOOK_SECRET`.
3. Set `NANGO_API_KEY` (it needs the connect-sessions, connections:read and records:read scopes), restart, then go to **Integrations → Connect Gmail**.
4. Google restricted scopes need OAuth verification or a security assessment before customer-wide onboarding. Nango doesn't do that for you.

## Before a real pilot

1. Set up an inbound domain in Resend, point the webhook at `POST /webhooks/resend`, and set `RESEND_*` and `INBOUND_DOMAIN`.
2. Choose the OIDC provider, set `AUTH_MODE=oidc` and `OIDC_*`, and add the OIDC login flow to the web app (it only has dev sign-in today).
3. Get a TypeSafe key, check the Jev response shape, and label at least 50 real emails to measure accuracy.
4. Choose an OpenRouter model that supports `json_schema`, and confirm the data-retention terms.
5. Turn on disk encryption, encrypted backups, and a tested restore of the SQLite file.
