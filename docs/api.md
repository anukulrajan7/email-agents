# API contract (v1)

Checked-in DTO contract shared by `apps/api` (Rust) and `apps/web` (TypeScript).
The TypeScript mirror lives in `apps/web/src/api/types.ts` — change both together.

## Conventions

- Base URL: `/api/v1`. Vite dev server proxies `/api`, `/health`, `/webhooks` to `http://localhost:8080`.
- Auth: `X-Auth-Token: <token>` (preferred; leaves `Authorization` free for a proxy password gate) or `Authorization: Bearer <token>`.
  - `AUTH_MODE=dev` (local only): token is `dev:<email>`, e.g. `dev:admin@acme.test`. The user is created on first call.
  - `AUTH_MODE=oidc`: token is an OIDC access/ID token (RS256) validated against `OIDC_ISSUER_URL` JWKS and `OIDC_AUDIENCE`.
- Workspace ID in the URL is only a requested resource; membership is enforced server-side (403 `forbidden` otherwise).
- Timestamps: RFC 3339 strings (UTC). IDs: opaque strings.
- Errors: `{ "error": { "code": string, "message": string, "request_id": string } }`.
  Codes: `unauthorized` (401), `forbidden` (403), `not_found` (404), `bad_request` (400), `conflict` (409),
  `not_implemented` (501), `email_not_verified` (403), `rate_limited` (429), `ai_unavailable` (503, the AI call failed),
  `internal` (500).

## Enums

```text
Role              = "admin" | "operator"
SourceKind        = "forwarding" | "nango_gmail" | "nango_microsoft" | "future"
SourceStatus      = "pending" | "connected" | "degraded" | "disconnected"
ProcessingStatus  = "pending" | "normalized" | "classified" | "workflow_running" | "workflow_completed" | "processed" | "failed"
Category          = "timesheet" | "leave" | "candidate_referral" | "approval" | "payroll" | "general" | "other"
Priority          = "low" | "normal" | "high"
ClassSource       = "jev" | "llm" | "manual" | "fallback"
DraftStatus       = "suggested" | "edited" | "approved" | "scheduled" | "sent"   // "scheduled" = send pending (undo-able)
ReviewStatus      = "open" | "closed"
RunStatus         = "running" | "completed" | "failed" | "skipped"
JobStatus         = "pending" | "running" | "succeeded" | "dead"
```

## Endpoints

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| GET | `/health/live` | – | – | `{status:"ok"}` |
| GET | `/health/ready` | – | – | `{status:"ok"}` or 503 |
| GET | `/api/v1/me` | any | – | `{user:{id,email}, workspaces: Workspace[], auth_mode:"dev"\|"oidc"}` |
| GET | `/api/v1/workspaces` | any | – | `{items: Workspace[]}` |
| POST | `/api/v1/workspaces` | any | `{name, packs?}` | `Workspace` (caller becomes admin; labels and presets of the chosen packs seeded; see Packs) |
| POST | `/api/v1/workspaces/:ws/members` | admin | `{email, role}` | `{user_id, email, role}` |
| GET | `/api/v1/workspaces/:ws/sources` | member | – | `{items: Source[]}` |
| POST | `/api/v1/workspaces/:ws/sources/forwarding` | admin | – | `Source` (idempotent: returns existing forwarding source) |
| POST | `/api/v1/workspaces/:ws/sources/:id/test-email` | admin | `{from?, subject?, body_text?}` | `{status:"queued"}` (only if `ALLOW_TEST_EMAILS=true`, else 404) |
| POST | `/api/v1/workspaces/:ws/sources/nango-session` | admin | `{provider: "gmail"\|"microsoft"}` | `{token, expires_at, provider_config_key}`; 501 if Nango unconfigured or provider is microsoft |
| POST | `/api/v1/workspaces/:ws/sources/nango-connections` | admin | `{connection_id, provider_config_key}` | `Source` (server re-verifies connection tags with Nango; 403 if not this workspace/user) |
| POST | `/api/v1/workspaces/:ws/sources/:id/sync` | admin | – | 202 `{status:"queued"}`; 409 for forwarding or disconnected sources |
| POST | `/api/v1/workspaces/:ws/sources/:id/disconnect` | admin | – | `Source` (status `disconnected`; Nango connection revoked best-effort) |
| GET | `/api/v1/workspaces/:ws/messages?cursor=&category=&q=&limit=&sort=&direction=` | member | – | `{items: MessageSummary[], next_cursor: string\|null}` |
| GET | `/api/v1/workspaces/:ws/messages/:id` | member | – | `MessageDetail` |
| PATCH | `/api/v1/workspaces/:ws/messages/:id/classification` | member | `{category, priority?, requires_reply?}` | `Classification` |
| POST | `/api/v1/workspaces/:ws/messages/:id/draft` | member | optional `{quality?: "premium"}` ("Write a better draft"); no body still works | 202 `{status:"queued"}`; 400 on an unknown `quality` |
| GET | `/api/v1/workspaces/:ws/reply-settings` | member | – | `ReplySettings` + `default_reply_style` |
| PUT | `/api/v1/workspaces/:ws/reply-settings` | admin | `ReplySettings` | `ReplySettings` |
| POST | `/api/v1/workspaces/:ws/messages/:id/reprocess` | admin | – | 202 `{status:"queued"}` |
| PATCH | `/api/v1/workspaces/:ws/drafts/:id` | member | `{body_text?, status?: "edited"\|"approved"}` | `Draft` |
| GET | `/api/v1/workspaces/:ws/reviews?status=` | member | – | `{items: ReviewItem[]}` |
| PATCH | `/api/v1/workspaces/:ws/reviews/:id` | member | `{status}` | `ReviewItem` |
| GET | `/api/v1/workspaces/:ws/workflows` | member | – | `{items: Workflow[]}` |
| PATCH | `/api/v1/workspaces/:ws/workflows/:id` | admin | `{enabled}` | `Workflow` (409 if `available=false`) |
| GET | `/api/v1/workspaces/:ws/workflow-runs` | member | – | `{items: WorkflowRun[]}` |
| GET | `/api/v1/workspaces/:ws/jobs?status=dead` | admin | – | `{items: Job[]}` |
| POST | `/api/v1/workspaces/:ws/jobs/:id/retry` | admin | – | `Job` |
| POST | `/webhooks/resend` | signature | raw Svix-signed body | `{status:"accepted"}` |
| POST | `/webhooks/nango` | `X-Nango-Hmac-Sha256` | raw body | `{status:"accepted"\|"duplicate"\|"ignored"}` |

## Shapes

```ts
type Workspace = { id: string; name: string; role: Role; created_at: string };

type Source = {
  id: string; kind: SourceKind; status: SourceStatus;
  inbound_address: string | null;          // forwarding only
  capabilities: { read: boolean; sync: boolean; provider_labels: boolean; provider_drafts: boolean; send: boolean };
  last_synced_at: string | null; last_error: string | null; created_at: string;
};

type MessageSummary = {
  id: string; subject: string; from: string; original_from: string | null;
  received_at: string; processing_status: ProcessingStatus;
  category: Category | null; priority: Priority | null; needs_review: boolean;
  attachment_count: number;
};

type Classification = {
  category: Category; priority: Priority; requires_reply: boolean;
  confidence: number | null; needs_review: boolean; source: ClassSource;
  model_id: string | null; updated_at: string;
};

type Draft = { id: string; message_id: string; status: DraftStatus; body_text: string; model_id: string | null; created_at: string; updated_at: string;
  send_at: string | null;      // while status = "scheduled": when it goes out
  send_error: string | null }; // why the last scheduled send didn't go out

type Attachment = { id: string; name: string; media_type: string | null; size_bytes: number | null; scan_status: string };

type AuditEvent = { id: string; actor_type: "user" | "system"; actor_id: string | null; action: string; entity_type: string; entity_id: string; created_at: string };

type MessageDetail = {
  id: string;
  source: { id: string; kind: SourceKind; can_create_provider_draft: boolean };
  from: string; original_from: string | null; to: string[];
  subject: string; body_text: string;            // plain text only; HTML is never returned for rendering
  received_at: string; processing_status: ProcessingStatus;
  attachments: Attachment[];
  classification: Classification | null;
  draft: Draft | null;
  review: { id: string; status: ReviewStatus; kind: string } | null;
  workflow_run: { id: string; status: RunStatus; error: string | null } | null;
  audit: AuditEvent[];                           // oldest first
};

type ReviewItem = {
  id: string; kind: string; title: string; status: ReviewStatus;
  message_id: string; message_subject: string; assigned_user_id: string | null; created_at: string;
};

type Workflow = {
  id: string; template_key: "timesheet_review" | "leave_request" | "candidate_referral";
  name: string; description: string; enabled: boolean;
  available: boolean;                            // false = P1 template, cannot be enabled yet
  last_run: { status: RunStatus; created_at: string } | null;
};

type WorkflowRun = { id: string; workflow_id: string; template_key: string; message_id: string; message_subject: string; status: RunStatus; error: string | null; created_at: string };

type Job = { id: string; kind: string; status: JobStatus; attempts: number; max_attempts: number; last_error: string | null; run_at: string; updated_at: string };
```

## Agent memory (v1)

Memory the agent uses when classifying and drafting. Two layers:
- **Contacts** — profiles learned automatically from every ingested email (no LLM): name, counts, usual categories.
- **Memories** — short notes. `source:"user"` = saved by a person (active immediately). `source:"learned"` =
  proposed by the agent from operator draft edits; arrives as `status:"suggested"` and is only used after a
  person accepts it (`PATCH status:"active"`). Archived memories are never used.

```text
MemoryScope   = "workspace" | "contact"
MemoryKind    = "fact" | "preference" | "style"
MemoryStatus  = "active" | "suggested" | "archived"
MemorySource  = "user" | "learned"
```

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| GET | `/api/v1/workspaces/:ws/memories?status=&scope=&contact=&q=` | member | – | `{items: Memory[]}` (default `status=active`; `q` = full-text search) |
| POST | `/api/v1/workspaces/:ws/memories` | member | `{scope, contact_email?, kind, content}` | `Memory` (201) |
| PATCH | `/api/v1/workspaces/:ws/memories/:id` | member | `{content?, kind?, status?}` | `Memory` |
| GET | `/api/v1/workspaces/:ws/contacts?q=` | member | – | `{items: Contact[]}` (most recent first, max 100) |
| GET | `/api/v1/workspaces/:ws/contacts/:email` | member | – | `{contact: Contact, memories: Memory[], recent_messages: MessageSummary[]}` |

`content` is 1–500 characters. `contact_email` is required when `scope="contact"` and must be absent for `"workspace"`.

```ts
type Memory = {
  id: string; scope: MemoryScope; contact_email: string | null; kind: MemoryKind; content: string;
  status: MemoryStatus; source: MemorySource; created_by: string | null;
  evidence_message_id: string | null;     // learned memories: the message whose draft edit taught it
  created_at: string; updated_at: string;
};

type Contact = {
  email: string; display_name: string | null; message_count: number;
  first_seen_at: string; last_seen_at: string;
  top_categories: { category: Category; count: number }[];   // up to 3, most frequent first
};
```

`MessageDetail` gains:
- `contact: Contact | null` — profile of the sender (`from`)
- `contact_memories: Memory[]` — active memories for that sender

`Draft` gains `memories_used: { id: string; content: string }[]` — what the drafter was given.

### Self-learning memory (wave 2, F1)

Memory **learns itself**; people only pin, confirm or delete. Sources:
- **Incoming mail** (job `learn_from_message`, after classification, efficient model): only `contact` facts
  (who someone is: name, role, company, which client) and `client` facts (open roles, process rules). Skipped:
  mail labelled `other` with no reply needed, automated senders (`no-reply@`, `notifications@`, …), short bodies.
- **The team's replies** (job `learn_from_reply`, when a draft is approved or sent, drafting model; and mail
  sent from the connected mailbox): also standing answers (`"Timesheets are due by the 3rd"`) and team
  preferences/style.

Filters (deterministic, after the model): one-off details are excluded by the prompt; anything mentioning
secrets/OTPs/passwords, bank/ID numbers, salary, health or other sensitive personal data, links, or long digit
runs is dropped (word-boundary matching); instructions phrased as facts (`"always send rates to…"`) are dropped;
at most 5 facts per email. Email text is fenced as untrusted data.

Confidence: facts seen only in outside mail are capped at **0.7**; once a team reply states them, **0.95**; each
extra distinct source adds 0.1 within the cap; a person confirming (or editing the text) sets **1.0**.
**Drafts only use manual memories and auto memories with confidence ≥ 0.6**; every use bumps `use_count` /
`last_used_at`.

Dedupe: an auto fact's identity is its normalised `subject|key` (e.g. `priya@acme.com|role`). Same identity →
the memory is updated (same text: confidence raised; changed text: newest wins) and the source is appended
(up to 10). Pinned or confirmed memories keep their text; manual memories are never changed; a deleted
(archived) auto memory is never relearned. Cap: 500 active auto memories per workspace; beyond that the
least-used, lowest-confidence, oldest unpinned unconfirmed ones are evicted.

`Memory` adds:
```ts
origin: 'auto' | 'manual';        // auto = learned (emails, replies, draft edits); manual = written by a person
confidence: number;               // 0..1 (manual = 1)
source_message_id: string | null; // latest email it was learned from (same value as evidence_message_id)
pinned: boolean;
use_count: number;
last_used_at: string | null;
```
- `GET /memories?origin=auto|manual` (400 for anything else; combines with `status`, `scope`, `contact`, `q`).
- `PATCH /memories/:id` also accepts `{pinned: boolean}` and `{confirmed: true}` (confidence 1; a `suggested`
  memory becomes `active`). `confirmed: false` → 400. Accepting a suggestion (`status: "active"`) also sets
  confidence 1. Delete = `status: "archived"`.

## Auth: email + password (v1)

`AUTH_MODE=password` (production). `AUTH_MODE=dev` (local only) keeps the `dev:<email>` token; the server refuses
to start with it unless `ALLOW_DEV_AUTH=1`, `PUBLIC_WEB_URL` is not https and the bind address is loopback. The client
always sends its token as `X-Auth-Token`. Sessions are opaque random tokens (only a SHA-256 hash is stored), 7-day
sliding expiry (renewed on use).

| Method | Path | Auth | Body | Response |
|---|---|---|---|---|
| GET | `/api/v1/auth/config` | – | – | `{mode: "password"\|"dev"\|"oidc", signup_enabled: boolean}` |
| POST | `/api/v1/auth/signup` | – | `{email, password, name?}` | 201 `{token, expires_at, user: {id, email, name}}` |
| POST | `/api/v1/auth/login` | – | `{email, password}` | `{token, expires_at, user: {id, email, name}}` |
| POST | `/api/v1/auth/logout` | token | – | 204 (revokes this session) |

Rules and errors:
- Password 10–128 characters. Email normalized to lowercase. `name` optional, ≤ 100 characters.
- Signup is by invitation: allowed for emails in `SIGNUP_ALLOWED_EMAILS`, domains in `SIGNUP_ALLOWED_DOMAINS`,
  emails an admin invited (`POST /workspaces/{ws}/members` for an address without a verified account → 202
  `{invited: true}`; the membership is created when that person signs up and verifies), and, only when both lists
  are empty, the very first account. Otherwise 403 `forbidden` "Sign-up is by invitation".
- Signup: 409 `conflict` if the email already has a *verified* account. An unverified account is taken over by the
  new sign-up (new password; old codes and sessions revoked), so nobody can pre-claim an address. The first
  successful verification revokes all other sessions.
- Login: always the same 401 `unauthorized` "invalid email or password" (no account enumeration);
  429 `rate_limited` after 10 failed attempts for that email from the same IP (15-minute window). Failures for one
  email across many IPs only slow attempts down (≤ 5 s), never lock the owner out.
- All auth endpoints except `config`/`logout`: 429 `rate_limited` above 10 requests/min per client IP (burst 20).
  `X-Forwarded-For` is honoured only from a loopback peer (the local reverse proxy).

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/v1/auth/password-reset/request` | `{email}` | 200 `{status:"sent"}` always; emails a 6-digit code (≤ 1/min, ≤ 5/hour per email) |
| POST | `/api/v1/auth/password-reset/confirm` | `{email, code, new_password}` | 200 `{status:"password_reset"}`; all sessions revoked (sign in again); 400 invalid code or password |
- `GET /api/v1/me` returns `user.name` too, and `auth_mode`.

## Email verification (OTP)

Enabled when SMTP is configured (`SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM`).
Codes are 6 digits, valid 10 minutes, max 5 attempts, stored only as a hash.

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/v1/auth/signup` | as before | **202** `{verification_required: true, email}` when verification is on (no token yet); 201 session otherwise |
| POST | `/api/v1/auth/verify` | `{email, code}` | `{token, expires_at, user}` (signs the user in); 400 `bad_request` "invalid or expired code"; 429 `rate_limited` |
| POST | `/api/v1/auth/resend-code` | `{email}` | 202 `{status:"sent"}` (no account enumeration; at most one code per 60 s) |
| POST | `/api/v1/auth/login` | as before | **403 `email_not_verified`** if the password is right but the email is unverified (a fresh code is sent) |

`GET /api/v1/auth/config` gains `email_verification: boolean`.

Delivery: every OTP and alert email is tried up to 3 times (backoff 0.4 s, 1.2 s) and recorded in `email_outbox`
(`kind`, `to_address`, `status` sending|sent|failed, `attempts`, `last_error`; OTP subjects are stored as "Verification code").
When a verification email can't be sent, `signup`, `login` (unverified) and `resend-code` return
**503 `email_not_sent`** ("we couldn't send the email to …") instead of pretending a code is on its way.

## Labels (workspace-defined categories)

Every workspace has its own labels. The AI classifies each email into exactly one active label. A label's
`description` is both the help text shown to people and the instruction given to the AI, so it should read like
a brief to a new teammate ("Invoices, receipts and payment reminders from vendors"). The system label `other`
always exists and cannot be archived.

`Category` in all earlier shapes is now a **label key** (`string`), e.g. `"timesheet"`, `"sales_lead"`, `"other"`.

```ts
type Label = {
  id: string; key: string; name: string; description: string;
  color: "slate" | "sage" | "plum" | "ochre" | "teal" | "postmark" | "ink" | "rule";
  position: number; system: boolean; archived: boolean;
  message_count: number;              // messages currently classified with this label
};
type LabelTemplate = "hr" | "general" | "blank";
```

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| GET | `/api/v1/label-templates` | any | – | `{items: {key: LabelTemplate, name, description, labels: {key,name,description}[]}[]}` |
| POST | `/api/v1/workspaces` | any | `{name, packs?: string[], goal?, template?: LabelTemplate}` (default `packs: ["general"]`; `template` is legacy: `hr` = staffing pack) | `Workspace` |
| GET | `/api/v1/workspaces/:ws/labels?include_archived=` | member | – | `{items: Label[]}` ordered by `position` |
| POST | `/api/v1/workspaces/:ws/labels` | admin | `{name, description, color?}` | `Label` (key = slug of name; 409 if taken) |
| PATCH | `/api/v1/workspaces/:ws/labels/:id` | admin | `{name?, description?, color?, archived?, position?}` | `Label` (409 when archiving `other` or a label used by an enabled workflow) |

Rules: `name` 1–40 chars, `description` 10–300 chars, max 30 active labels per workspace.
`PATCH .../messages/:id/classification` and `GET .../messages?category=` validate against the workspace's active labels.
Workflow templates attach to a label key (`timesheet_review` → `timesheet`); `Workflow` gains `label_key: string`
and `available` is false when the workspace has no such active label.

## Packs (use-case bundles)

A pack bundles labels, automation presets, a default reply style and features for one kind of work. New
workspaces get `general` unless they choose otherwise. Packs: `general` (default business inbox), `support`,
`sales`, `finance`, `personal`, `staffing` (HR and recruiting). The **staffing** pack alone owns the HR labels
(timesheet, leave, candidate referral, client requirement, payroll), the built-in HR workflows, CV and
requirement extraction, candidate matching, requirement-agent cases and the "HR Operations" sign-off. Without it
none of these run. Existing workspaces were backfilled onto `staffing` (migration `0032_packs.sql`).
Turning a pack on adds its missing labels (archived labels stay archived; 409 if the 30-active-label cap would be
exceeded). Turning it off deletes nothing; it only stops the pack's features.

```ts
type Pack = {
  key: "general" | "staffing" | "support" | "sales" | "finance" | "personal";
  name: string; description: string;
  labels: { key: string; name: string; description: string }[];
  presets: string[];                 // automation presets offered
  features: string[];                // "matching" | "requirement_agent" | "hr_workflows" (staffing only)
};
type Workspace = { id; name; role; created_at; packs: string[] };   // packs also on GET /me and GET /workspaces
```

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| GET | `/api/v1/packs` | any | – | `{items: Pack[]}` |
| GET | `/api/v1/workspaces/:ws/packs` | member | – | `{enabled: string[], items: (Pack & {enabled: boolean})[]}` |
| PUT | `/api/v1/workspaces/:ws/packs` | admin | `{packs: string[]}` (at least one; 400 on unknown key) | same as GET |

`POST /workspaces` accepts `packs` (same validation) and an optional `goal` (what the workspace is for, max 1,000
chars). Reply defaults follow the packs: staffing keeps the HR reply style and "HR Operations" sign-off, every
other pack uses a neutral style and signs off with the workspace name.

## Matching: referrals ↔ client requirements (P1)

Emails labelled `client_requirement` become **Requirements**; emails labelled `candidate_referral` (and their
PDF/DOCX CVs) become **Candidates**. The AI proposes; a recruiter shortlists or dismisses. Re-scoring never
overrides a recruiter's decision.

**Semantic matching (M4).** Skills are normalised by a taxonomy (`src/skills.rs`, ~80 skills: `reactjs`/`react.js` →
`react`, `springboot` → `spring boot`, `k8s` → `kubernetes`, `golang` → `go`, `postgres` → `postgresql`, `mssql` →
`sql server`, `dotnet` → `.net`, `ml` → `machine learning`, …) at extraction and at scoring. Each record gets a
profile embedding (`baai/bge-m3`; role, skills, experience, location, summary + an excerpt of the email/CV/JD text),
re-embedded only when its profile changes. A new or updated record **recalls** the 20 nearest open counterparts in its
workspace (vector KNN) plus every open counterpart sharing a skill (exact, alias or related), then scores each pair:

| Component | Points | Rule |
|---|---|---|
| Must-have skills | 50 | Share of must-haves the candidate has; a **related** skill (e.g. `spring boot` ↔ `spring`, `next.js` ↔ `react`, `postgresql` ↔ `sql`) counts half |
| Experience | 20 | Fits the band 20 · ≤1 yr below or above 10 · further below 0 · unknown 6 · no band 12 |
| Location | 15 | Same city or remote role 15 · different 3 · unknown 8 |
| Semantic fit | 15 | Cosine similarity of the two profiles: 0 at ≤ 0.40, 15 at ≥ 0.80, linear between · no embedding 7.5 |

Matches scoring ≥ 40 are kept. Reasons now include `Has related skill: next.js (for react)` and one of
`Profile closely matches the role (similarity 0.82)` (≥ 0.65), `Profile is somewhat similar to the role (similarity
0.58)` (≥ 0.50) or `Profile is not much like the role (similarity 0.31)` (`ok: false`). If the embedding provider is
down, the match job retries; on its last attempt it scores skill-overlap counterparts without semantic fit. Response
shapes are unchanged.

**Daily match digest.** Once per UTC day at the alert settings' `digest_hour`, the alert `recipients` get one email with
the new strong matches (status `suggested`, score ≥ 70, created in the last 24 hours, requirement open), grouped by
requirement with links to `{APP_BASE_URL}/w/:ws/matching?req=:requirement_id`. Nothing is sent when there are none;
a day is claimed once (`match_digests`), so retries never send twice. Turned off with the alert `daily_digest` switch.

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| POST | `/api/v1/workspaces/:ws/matching/setup` | admin | – | `{labels: ["client_requirement","candidate_referral"]}` (creates or restores both labels) |
| GET | `/api/v1/workspaces/:ws/matching/requirements` | member | – | `{items: Requirement[]}` |
| PATCH | `/api/v1/workspaces/:ws/matching/requirements/:id` | member | `{status: "open"\|"closed"}` | `{id, status}` |
| GET | `/api/v1/workspaces/:ws/matching/candidates` | member | – | `{items: Candidate[]}` |
| GET | `/api/v1/workspaces/:ws/matching/matches?requirement_id=&candidate_id=&status=` | member | – | `{items: Match[]}` (score desc; dismissed hidden unless `status=dismissed`) |
| PATCH | `/api/v1/workspaces/:ws/matching/matches/:id` | member | `{status: "suggested"\|"shortlisted"\|"dismissed"}` | `Match` |

```ts
type Requirement = {
  id: string; message_id: string; client: string | null; role_title: string;
  must_have_skills: string[]; nice_to_have_skills: string[];
  min_experience: number | null; max_experience: number | null; location: string | null; positions: number | null;
  status: "open" | "closed"; source: "llm" | "rules" | "manual"; created_at: string;
  match_count: number; top_score: number | null;
};
type Candidate = {
  id: string; message_id: string; name: string; email: string | null; skills: string[];
  total_experience: number | null; location: string | null; notice_days: number | null;
  referred_by: string | null; summary: string | null; status: "active" | "placed" | "archived";
  source: "llm" | "rules" | "manual"; created_at: string; match_count: number;
};
type Match = {
  id: string; score: number; status: "suggested" | "shortlisted" | "dismissed";
  reasons: { ok: boolean; text: string }[];
  requirement: { id: string; role_title: string; client: string | null; location: string | null };
  candidate: { id: string; name: string; total_experience: number | null; location: string | null; notice_days: number | null };
  created_at: string; updated_at: string;
};
```

## Automations (when X, do Y)

All under `/api/v1/workspaces/{ws}`. Operators read, admins write. Trigger `email_received` fires after an email is
classified and understood (insight). Conditions are AND-ed. Each automation runs at most once per message.

| Method | Path | Role | Result |
|---|---|---|---|
| GET | `/automations` | Operator | `{ items: Automation[] }` (seeds the two default automations the first time a pre-existing workspace looks) |
| POST | `/automations` | Admin | 201 `Automation` (body `AutomationInput`) |
| PATCH | `/automations/{id}` | Admin | `Automation` (partial `AutomationInput`) |
| DELETE | `/automations/{id}` | Admin | 204 |
| GET | `/automations/presets` | Operator | `{ items: Preset[] }` |
| POST | `/automations/presets/{key}` | Admin | 201 `Automation` created from the preset (404 unknown key) |
| GET | `/automations/{id}/runs?limit=50` | Operator | `{ items: AutomationRun[] }` newest first (limit 1-200) |
| POST | `/automations/{id}/test` | Operator | `{ matched: [{ message_id, subject }] }` dry run over the last 50 messages, no side effects |

```ts
type Condition =
  | { field: "label"; op: "is" | "is_not"; value: string }            // label key
  | { field: "priority"; op: "is"; value: "high" | "normal" | "low" }
  | { field: "needs_reply"; op: "is"; value: boolean }                 // insight, falling back to the classifier
  | { field: "from"; op: "contains"; value: string }                   // case-insensitive
  | { field: "subject"; op: "contains"; value: string };
type Action =
  | { type: "draft_reply"; instructions?: string }                     // reuses the message's existing draft
  | { type: "save_to_gmail_drafts" }                                    // needs draft_reply before it
  | { type: "send_reply"; daily_cap: number; allowed_domains: string[] } // needs draft_reply before it; guarded
  | { type: "create_review"; title?: string }
  | { type: "remind_if_no_reply"; hours: number };                      // review item kind "reminder" if still unanswered
type AutomationInput = { name: string; enabled: boolean; conditions: Condition[]; actions: Action[] };
type Automation = AutomationInput & { id: string; trigger: "email_received"; preset_key: string | null;
  run_count: number; last_run_at: string | null; created_at: string; updated_at: string };
type Preset = { key: string; name: string; description: string; automation: AutomationInput };
type AutomationRun = { id: string; automation_id: string; message_id: string; message_subject: string;
  status: "completed" | "skipped" | "failed";
  actions: { type: string; status: "done" | "skipped" | "failed"; detail: string | null }[]; created_at: string };
```

Limits: name 1-100 chars, 1-10 actions, at most 10 conditions, `daily_cap` 1-1000, `hours` 1-720, at most 50
automations per workspace. Any violation, unknown field/op/action type, `save_to_gmail_drafts` or `send_reply` without an
earlier `draft_reply`, or an empty `allowed_domains` returns 400 `invalid_automation`.

Presets: `reply_to_questions` (needs_reply -> draft + save to Gmail Drafts), `acknowledge_timesheets` (label timesheet ->
draft + save), `chase_important` (priority high + needs_reply -> remind after 4h), `auto_ack_referrals` (label
candidate_referral -> draft + save). New workspaces get `reply_to_questions` and `chase_important`, enabled. No preset sends.

`send_reply` safety (any failed check records the action as `skipped` with a reason): the draft has no `[NEEDS: ...]` gap
and no unanswered items; the sender's domain is in `allowed_domains`; the classification is manual or has confidence >= 0.85
and did not come from the error fallback; today's sends for this automation are below `daily_cap`; the source can send; the
sender is not the mailbox itself or an automated address. Sends are audited as `automation.sent_reply`.
Manual `POST /drafts/{id}/send` is also refused with 409 `send_blocked` while the body contains `[NEEDS:` (see Send safety).
An automated send also needs the AI review to run; if it can't, the send is held (`ai_review_unavailable`).

`remind_if_no_reply` schedules a delayed job. When due, if the draft was not sent and no later message from someone else
arrived in the thread, a review item of kind `reminder` ("No reply yet: ...") is opened.

## Search (knowledge store)

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| GET | `/api/v1/workspaces/:ws/search?q=&limit=10` | member | – | `{items: SearchItem[]}` |

```ts
type SearchItem = {
  kind: "email" | "attachment";  // where the match was: the email body or an attachment's text
  message_id: string;
  subject: string;
  snippet: string;               // ~200 chars around the first matching word ("… " marks a cut)
  score: number;                 // reciprocal-rank-fusion score; only meaningful for ordering
};
```

- Hybrid: keyword (FTS5 BM25) and meaning (vector KNN, `baai/bge-m3`) results merged by reciprocal rank fusion.
  Meaning search always returns the nearest items, so a query with no keyword match still gets ranked results.
- One item per `(kind, message_id)`, best match first. `limit` 1–50 (default 10); blank `q` → `{items: []}`.
  `q` is plain text (FTS operators are treated as words), max 500 chars.
- Scoped to the workspace; approved/sent replies are indexed for drafting but never returned here.
- Indexing is asynchronous (after classification), so a brand-new email appears a few seconds later.

## Wave 2: understanding, ordering, clean text, direction

### Processing status
`processed` is the terminal state for a message that was classified and that no workflow matched (nothing else
to do). `workflow_completed` is kept for workflow runs. Mail the connected mailbox itself sent (see Direction)
is stored as `processed` straight away. `workflow_running` is reserved.

### Message list: `GET .../messages`
Query additions: `sort=recent|importance` (default `recent`) and `direction=inbound|outbound|all` (default
`inbound`; anything else is 400). `sort=importance` orders by needs-reply first, then priority
(high, normal, low), then newest. Its `cursor` is an opaque offset token (`next_cursor` as before); a cursor from
one sort is not valid for the other. `category` and `q` filters work with both sorts.

`MessageSummary` adds (all additive):
```ts
{ direction: "inbound" | "outbound";
  needs_reply: boolean | null;   // insight.needs_reply, else classification.requires_reply, else null
  summary: string | null }       // insight summary, cut to 140 chars (last char "…" when cut)
```
`priority` (existing) is `"high"|"normal"|"low"|null`.

### Message detail: `GET .../messages/:id`
Adds:
```ts
{ insight: { summary: string; asks: string[]; deadline: string | null; needs_reply: boolean; missing_info: string[] } | null;
  from_name: string | null;        // sender display name ("Priya Sharma"); `from` stays the address
  cc: string[];                    // `to` lists To recipients only (older rows may still hold To+Cc, Cc is removed)
  direction: "inbound" | "outbound";
  draft: { ...; unanswered: string[] } | null }   // asks the draft could not answer; each is "[NEEDS: ...]" in body_text
```
Already present and used for the Gmail-style header: `from`, `to`, `received_at` (ISO 8601).
`body_html_safe` is filled for every Gmail message whose original has an HTML part (older rows are re-fetched, see below).
`model_id` fields stay in the API; the UI never shows them.

### Text cleaning
`body_text` is cleaned at ingest for every source (forwarding, Gmail): HTML comments (including Outlook conditional
comments) and stray tags are removed, `<https://...>` wrappers become bare URLs on the same line as their anchor
text, runs of blank lines collapse to one, trailing spaces are trimmed, an immediately repeated block is dropped,
and when the text part is empty or junk it is derived from the HTML part. Stored rows are re-cleaned once by the
`clean_stored_text` job (queued by the worker's maintenance tick while any row has `text_clean_v = 0`).

### Gmail HTML re-fetch
Legacy Gmail rows stored without HTML are re-fetched once in full format (up to 200 per run, at most 3 tries per
message, enqueued after each sync as `refetch_gmail_html`). It fills `body_html`, re-cleans the text and refreshes
sender name and Cc. The Gmail MIME walk is recursive (multipart/alternative, related, mixed).

### Direction (inbound / outbound)
The Gmail queries also return the mailbox's own sent mail. A message is `outbound` when Gmail's `SENT` label is
present or its sender is the connected mailbox address (`email_sources.mailbox_address`, learned from the Gmail
profile at sync; rows stored earlier are marked on that sync). Outbound messages are stored as evidence of
replies but never classified, analysed, drafted against or run through workflows, do not create contacts, and are
hidden from the default list. Server helper for other features:
`threads::thread_has_reply_after(db, workspace_id, thread_key, after_ms) -> bool` is true when the thread
(`thread_key` = our thread id or the provider thread id) has an outbound message after `after_ms`, or a sent draft
for a message in it.

### Label suggestions: `POST /api/v1/workspaces/:ws/labels/suggest` (admin)
The AI reads up to 60 recent inbound emails (subject + snippet, unlabelled `other` first) and proposes up to 5 new labels.
```ts
{ items: { key: string; name: string; description: string;
           color: "slate" | "sage" | "plum" | "ochre" | "teal" | "postmark" | "ink";   // palette name, valid for POST /labels
           example_subjects: string[]; estimated_count: number }[] }
```
`key` is the slug of `name`, never equals an existing key (archived labels included). Accepting a suggestion is
`POST .../labels {name, description, color}`. No mail returns `{items: []}`. If the AI call fails: 503
`ai_unavailable`. Contract note: the original sketch used a hex colour; a palette name is returned so the result
can be sent to `POST /labels` unchanged.

### Label health: `GET /api/v1/workspaces/:ws/labels/health?days=30` (operator)
How well automatic labelling works over the last `days` days (1–180, default 30; otherwise 400) of **inbound** mail
(`direction = 'inbound'`, by `received_at`, UTC days).
```ts
{ days: number; total: number;            // classified inbound emails in the window
  other_share: number;                    // share landing in `other`
  auto_share: number;                     // share nobody had to change (extra field beyond the original sketch)
  labels: { key: string; name: string; count: number; share: number;
            auto: number;                 // count - corrected_into: the AI's pick stood
            corrected_away: number;       // the AI chose this label and a person changed it
            corrected_into: number;       // a person moved the email into this label
            correction_rate: number;      // corrected_away / (auto + corrected_away)
            avg_confidence: number | null;// mean of the AI's own picks (null with no model confidence)
            trend: { date: "YYYY-MM-DD"; count: number }[] }[];   // one point per day, zeros included
  other_trend: { date: string; count: number }[];
  advice: { kind: "add_label" | "label_unreliable" | "label_unused"; label_key: string | null; text: string }[] }
```
`labels` holds every non-`other` active label (plus archived ones that still have mail in the window); `other` is
reported through `other_share` / `other_trend`. Ratios are rounded to 2 decimals. Advice rules: `other_share >= 0.25`
-> `add_label` (listed first); `correction_rate >= 0.2` with at least 5 AI picks -> `label_unreliable`; an active
label with 0 emails in the window (and mail in the workspace) -> `label_unused`.

Correction signal: when a person changes an email's label (`PATCH .../messages/:id/classification`),
`classifications.ai_category` keeps what the AI chose first (set once, never overwritten by later human changes) and
`confirmed` becomes 1. AI re-runs never touch either column.

## Alerts (wave 2, F2)

Deterministic rules, evaluated every 15 minutes per workspace (job `evaluate_alerts`, scheduled by the worker).
Only mail received in the last 7 days and older than `unanswered_hours` can raise an alert.

| Kind | Fires when | Auto-resolves when |
|---|---|---|
| `important_unanswered` | priority `high`, or needs a reply (insight, else classifier) and label ≠ `other`; label not `approval`/`leave`; sender is not on the team; no reply yet | a reply exists, or the label/priority changes |
| `approval_pending` | label `approval` or `leave`; no closed review item; no reply yet | the review item is closed, or a reply exists |
| `automation` | raised by automations via `alerts::raise` | – (resolve by hand) |

**Reply detection** (until messages carry a direction): a later message in the same thread sent from a team
address (any workspace member's email, or the user who connected the mailbox), or a reply sent from Post Room
(`drafts.status = 'sent'`) in the thread after the email arrived. Mail sent by the team never raises alerts.

**Escalation**: an `open` alert unchanged for `escalate_after_hours` goes up one level (max 3), becomes
`severity: high` and is emailed to `escalate_to` (or `recipients` if empty). Snoozed alerts don't escalate.

**Email** (Hostinger SMTP, same transport as verification codes): pending alerts are batched into one email per
recipient per run (plain text + HTML), each with a deep link `{APP_BASE_URL}/w/{ws}/inbox/{message_id}`
(`/w/{ws}/alerts` when there is no message). **Daily digest**: once per UTC day, in the run that falls in
`digest_hour` (**UTC**), all open alerts go to `recipients` (skipped if none are open). `enabled: false` stops
new alerts, escalation and email; existing alerts still auto-resolve. A resolved alert is never re-raised.

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| GET | `/api/v1/workspaces/:ws/alerts?status=open\|resolved\|all` | member | – | `{items: Alert[], open_count: number}` (`open` = open + snoozed; `open_count` counts `open` only; max 200) |
| PATCH | `/api/v1/workspaces/:ws/alerts/:id` | member | `{status: 'resolved'}` / `{status: 'open'}` / `{status?: 'open', snooze_hours: 1-168}` | `Alert` |
| GET | `/api/v1/workspaces/:ws/alert-settings` | member | – | `AlertSettings` |
| PUT | `/api/v1/workspaces/:ws/alert-settings` | admin | `AlertSettings` (all fields) | `AlertSettings` (addresses trimmed, lower-cased, deduped) |
| POST | `/api/v1/workspaces/:ws/alert-settings/test` | admin | – | `{sent_to: string[]}`; 409 if email isn't configured, 400 with no recipients |

```ts
interface Alert { id: string; kind: 'important_unanswered' | 'approval_pending' | 'automation'; severity: 'high' | 'normal';
  title: string; body: string; message_id: string | null; message_subject: string | null;
  status: 'open' | 'snoozed' | 'resolved'; escalation_level: number; notified: string[];
  snoozed_until: string | null; created_at: string; resolved_at: string | null }
interface AlertSettings { enabled: boolean; recipients: string[]; escalate_to: string[];
  unanswered_hours: number;      // 1-168, default 4
  escalate_after_hours: number;  // 1-720, default 24
  daily_digest: boolean;         // default true
  digest_hour: number }          // 0-23 UTC, default 9
```
Defaults apply until an admin saves settings; until then `recipients` = the workspace admins' emails. Validation
errors → 400 `bad_request`; unknown fields → 422.

Config: `APP_BASE_URL` (default `https://mail.anukulraj.tech`) for links; SMTP via `SMTP_HOST`/`SMTP_PORT`/
`SMTP_USERNAME`/`SMTP_PASSWORD`/`SMTP_FROM` (without SMTP, alerts are in-app only).

## Replies: drafting tiers and workspace reply settings

### Reply settings

```ts
ReplySettings = {
  auto_reply: boolean;               // NULL in the DB = the server default (AUTO_DRAFTS)
  reply_instructions: string;        // max 2000 chars, trimmed; "" = use the default style
  signature: string | null;          // max 200 chars; blank -> null; replaces "HR Operations"
}
```

`GET .../reply-settings` (operator) also returns `default_reply_style: string`, the guidance drafts use while
`reply_instructions` is empty (a generic HR/staffing business context). `PUT` (admin) replaces all three fields;
over-long values return 400 `bad_request`, an operator gets 403. Audited as `reply_settings.updated`.

Effective automatic drafting is the workspace `auto_reply` when saved, otherwise `AUTO_DRAFTS`. It gates both the
workflow draft step and the `draft_reply` automation action (recorded `skipped` when off). A person clicking
Generate draft always drafts. `reply_instructions` and `signature` reach every draft as a trusted `<reply_style>` block
(they never override the safety rules). The drafter also receives the label's name and description.

### Drafting tiers

Each draft is written by a standard or a premium tier (internal, never shown in the UI). `ai::routing::draft_quality`
picks premium when the classification priority is `high`, the label is `client_requirement` or `candidate_referral`,
the insight has 3 or more asks or any missing info, or the request sent `{"quality":"premium"}`; otherwise standard.
The chosen tier is stored in the `draft.generated` audit metadata as `"tier": "standard" | "premium"`.

Config: `OPENROUTER_DRAFT_MODEL_ID` (standard tier, default `deepseek/deepseek-v4-flash`) and
`OPENROUTER_PREMIUM_MODEL_ID` (premium tier, default = `OPENROUTER_MODEL_ID`). If only one is set, both tiers use it.
Without `OPENROUTER_API_KEY` the template drafter is used for both.

## Agents (cases, steps, approvals)

Full contract: `docs/agents-contract.md`. All routes are under `/api/v1/workspaces/:ws`; Operators read, start cases
and decide approvals; Admins cancel. Other workspaces' ids return 404; non-members get 403.

| Method | Path | Role | Returns |
|---|---|---|---|
| GET | `/cases?status=&kind=` | operator | `{ items: CaseSummary[] }` (newest activity first, max 200; one status) |
| GET | `/cases/:id` | operator | `CaseDetail` |
| POST | `/cases` | operator | `CaseSummary`; body `{kind:'requirement', requirement_id}` or `{kind:'inbox', message_id}` |
| POST | `/cases/:id/cancel` | admin | `CaseSummary`; 409 if the case already ended; pending approvals become `declined` |
| GET | `/approvals?status=` | operator | `{ items: Approval[], pending_count }` (`status` optional: pending, approved, declined) |
| POST | `/approvals/:id` | operator | `Approval`; body `{decision:'approve'|'decline', edited_body?, note?}`; 409 once decided |

Shapes are as in the contract, with these details:

- `CaseSummary.steps` is the number of planner turns taken; in `CaseDetail`, `steps` is the `Step[]` timeline.
  `CaseDetail` also has `state` (case facts, e.g. `candidates`, `last_draft_id`, `new_message_id`), `approvals`,
  `status_note` and `failure_reason` (plain sentence, e.g. "used this case's AI budget of $0.10 without finishing").
- `Approval` adds `step_seq: number | null` (the timeline step that asked for it). `decided_by` is the person's email.
- `POST /cases` is idempotent: one case per requirement and per (kind, message); an existing case is returned.
  409 `conflict` when no planner model is configured; 404 for an unknown requirement or message.
- Step and approval `summary` strings are plain sentences; no API string names a model.

Runtime:

- **Triggers.** A requirement case starts when matching has run for a new `client_requirement` record (one per
  conversation: a client's reply that is re-read as a requirement does not start another). An inbox case starts when
  a new inbound email is understood as needing a reply, its label is not `other` or `client_requirement`, its
  conversation has no open case, and the workspace's effective auto-reply is on. Mail older than `AI_AUTO_HOURS`
  never starts a case automatically. Without a planner model nothing starts automatically.
- **Steps.** Each turn is one durable job `agent_step:<case>:<turn>` (kind `agent_step`, priority 3). The planner
  returns one tool call or `finish {outcome?, summary}`. `read` tools run at once; `low` and `high` tools are first
  checked by the critic: fixed rules (no `[NEEDS:]` gaps, only to an inbound sender of the case's conversation,
  never re-propose a declined draft, only matched candidates, at most 3) and then a cheap model (no invented facts,
  no promises of money/approvals/dates). A blocked check is a `critic` step with status `blocked`.
- **Approvals.** `send_email` / `submit_candidate` create a pending approval and the case is `waiting_approval`.
  Approve (optionally with `edited_body`, which updates the draft first) sends via the normal reply path
  (`replies::send_draft`, so its guards apply; a refused send is recorded as an error step) and, for submissions,
  marks the matches `shortlisted`. Decline records a note the planner sees. Both continue the case; audited as
  `agent_approval.requested|approved|declined`, plus `case.started|finished|failed|cancelled`.
- **Sleep and wake.** `schedule_check {hours}` sets `waiting_event` and `next_wake_at`; a new inbound email in the
  case's conversation wakes it at once (its id is in `state.new_message_id`).
- **Limits.** `AGENT_MAX_STEPS` (default 15) turns and `AGENT_BUDGET_USD` (default 0.10) per case; 3 error turns
  in a row (unusable tool call, unknown tool, bad arguments, tool failure) also fail the case. A turn whose AI call
  keeps failing through all job retries fails the case. Each step stores tokens and cost (provider-reported cost,
  else an estimate); `spent_usd` is the case total.
- **Models.** Planner `OPENROUTER_PLANNER_MODEL_ID` (default `deepseek/deepseek-v4-flash`); high-priority
  requirement cases use `OPENROUTER_PREMIUM_MODEL_ID`. Critic: the classifier model (else `OPENROUTER_MODEL_ID`).
  Requests disable reasoning and route only to providers with `data_collection: deny`.

## Send safety, undo send (wave 3, agent S)

Every outgoing reply (person, automation, agent approval) goes through `replies::send_draft`, which runs the safety
check and **schedules** the send `SEND_UNDO_SECONDS` (default 30, 0..600) ahead. The `send_draft` job re-runs the check at
`send_at`, then calls Gmail. Drafts can't be edited (409) while scheduled.

| Method | Path | Role | Body | Response |
|---|---|---|---|---|
| POST | `/api/v1/workspaces/:ws/drafts/:id/check` | operator | optional `{body_text}` (unsaved editor text) | `SafetyReport` (never sends) |
| POST | `/api/v1/workspaces/:ws/drafts/:id/send` | operator | `{confirm: true}` | `{status: "scheduled", draft_id, send_at}`; **409 `send_blocked`** with `error.blocking` / `error.warnings`; 409 `conflict` if already scheduled/sent |
| POST | `/api/v1/workspaces/:ws/drafts/:id/undo-send` | operator | – | `{status: "cancelled", draft_id}`; 409 once Gmail has been called ("too late") or if not scheduled |

```ts
interface Issue { code: string; message: string; fix: string | null }
interface SafetyReport { blocking: Issue[]; warnings: Issue[] }   // "Send" is disabled while blocking is non-empty
```

Blocking codes: `invalid_recipient`, `own_mailbox` (reply would go to our mailbox / the email was sent by us),
`automated_recipient` (no-reply, notifications, mailer-daemon …), `subject_mismatch` (must be `Re: <original>`),
`empty_body` / `too_short` (< 20 chars) / `too_long` (> 4000), `open_gaps` (`[NEEDS:`), `placeholder` (`{{x}}`, `{name}`,
`<name>`, `[Name]`, `XXX`, `TBD`), `duplicate` (a reply in the thread in the last 10 min, or another one scheduled),
`unsupported_promise` (judge: promises approvals/money/dates/decisions not in the email or team notes);
`ai_review_unavailable` blocks automated sends only.
Warning codes: `attachment_mentioned`, `unknown_contact` (links/phones not in the thread, team notes, reply style or
signature), `tone`, `misses_ask` (judge score < 0.5), `ai_review_unavailable` (person sends), `mailbox_unverified`.

No double send: each Gmail call carries our own `Message-ID` (`<snd_…@postroom.mail>`) recorded on the draft first.
A clear 4xx releases it; a timeout/5xx keeps it open, and before any retry the send is reconciled with Gmail
(`rfc822msgid:` search, then the thread: our Message-ID or a SENT message after the attempt started). Not found within
2 minutes = "unsure", no resend. A send that is blocked at send time, or gives up, clears the schedule, sets
`send_error` and raises a `automation` alert "Reply not sent: …".

### Judge (typed decisions)

`state.judge: Arc<dyn ai::judge::Judge>` answers a batch of typed questions in one cheap call:
Jev (`TYPESAFE_API_KEY`) → OpenRouter classifier model (strict JSON schema, reasoning off, `data_collection: deny`) → none.

```rust
async fn judge(&self, context: &str, questions: &[Question]) -> Result<Answers, AiError>;
// Question::yes_no(id, prompt) | Question::score(id, prompt) /* 0..1 */ | Question::choice(id, prompt, &[options])
// answers.yes_no(id) -> Option<bool>, .score(id) -> Option<f64>, .choice(id) -> Option<&str>, .reason(id) -> &str
// ai::judge::ask_cached(&db, judge, scope, context, questions) caches by (scope, material, questions).
```
Used for: the pre-send review (above); the `important_unanswered` alert gate (non-high-priority candidates only, cached,
fails open); needs-reply tie-break when the analyst and classifier disagree. Agent critics can call the same API.

## Agents: registry, system agents, routing, autonomy (WP-B)

Many agents per workspace. Contract types live in `src/agent_registry/spec.rs` (`AgentSpec`, `validate_basic`);
migration `0034_agent_registry.sql` adds `agents`, `agent_versions`, `automation_versions`, `message_routes` and the
`agent_id` columns on cases, approvals, automations and drafts. Nothing existing is rebuilt.

**Kinds and scopes.** `kind` is `lead` (one per workspace, Meera: routes and is the fallback), `system` (seeded from the
enabled packs or one per member) or `custom` (created or cloned by an admin). `scope` is `workspace` or `personal`.
**Every member has exactly one personal agent** (`scope: "personal"`, `owner_user_id`), seeded when they join and
backfilled for existing members. It works on that member's own mailbox: their profile, preferences and per-user memory.
A personal agent, its versions, outgoing mail, cases and approvals are **never visible to any other member, admins
included** (404). Workspace agents (lead, pack desks, HRMS desks) handle workspace-level processes.

**Seeding.** Enabling a pack seeds its desks (idempotent per `(workspace_id, key)`); disabling pauses them with
`paused_reason: "pack_off"` and re-enabling restores them. Nothing is deleted. Template agents start `active` at level 0
(observe) except the lead, the Inbox Assistant and personal agents (level 1).

| Pack | Agents (template key) |
|---|---|
| general (always) | Lead `general.lead`, Inbox Assistant `general.inbox` |
| support | Ticket Resolver `support.resolver`, Escalations `support.escalation` |
| sales | Lead Qualifier `sales.qualifier`, Follow-ups `sales.followups` |
| finance | Invoices & Payments `finance.invoices` |
| personal | Scheduling `personal.scheduling` |
| staffing (category `hrms`) | Leave `hrms.leave`, Timesheets `hrms.timesheets`, Onboarding `hrms.onboarding`, Candidates & Requirements `hrms.requirements`, Payroll queries `hrms.payroll` |
| every member | Personal agent `personal.mailbox` |

**Routing** (no AI call), `agent_registry::routing::route`, after a message is understood. Precedence:
1. `manual` (a person reassigned the message with `POST …/route`)
2. `thread` (an open case or an earlier route in the conversation)
3. `rule` (a trigger with domains, addresses, keywords, attachment types or priority; ties go to the most specific match, then the oldest agent)
4. `label` (a trigger with `labels`)
5. `personal` (mail in a member's own mailbox goes to their personal agent)
6. `generic` (a trigger with only `needs_reply`, e.g. the Inbox Assistant)
7. `fallback` (the lead)

Only `active` agents are routed to. An agent at effective level 0 does not take real work: the route is recorded with
`shadow: true`, an observing (shadow) case runs beside the real handler, and the next candidate at level 1 or more handles the mail.

**Autonomy levels.** Effective level = min(earned `autonomy_level`, admin `autonomy_cap`, spec `max_autonomy`, env
`AGENT_MAX_AUTONOMY` (default 3), the mailbox owner's profile: `always_ask` means at most 1).

| Level | Name | Behaviour |
|---|---|---|
| 0 | Observe | Shadow cases: no `drafts` row is written (the body stays in the step output) and send, submit and task actions are simulated |
| 1 | Draft | Real drafts. Every send and submission asks a person |
| 2 | Routine send | `send_email` goes without a person when the fixed rules pass (original sender, same conversation, known contact, no money or decision words, no `[NEEDS]`), the send-safety judge passes and the daily cap allows it |
| 3 | Autonomous | Level 2 plus replies to any current participant of the conversation and first replies to an inbound sender |

Fixed floor at every level (always asks a person): new recipients, forwards, outgoing attachments, `submit_candidate`,
`create_automation`, money or decision words, `[NEEDS]`, any `drafting.never_do` hit, and the **injection gate**: inbound
text in the conversation that contains a tool name (`send_email`, `submit_candidate` …) or an override phrase (`ignore
previous`, `system:`, `bcc`, `without approval` …) forces approval. Every automatic send at level 2 or more is held for
`AGENT_AUTO_SEND_DELAY_SECS` (default 600) using the draft's `send_at`; it shows in `GET …/agents/:id/outgoing` and
**Stop** is `POST …/drafts/:id/undo-send`. The lead keeps today's settings: `PUT …/agent` with `send: "auto"` lifts the
lead and the personal agents to level 2, any other value caps them at 1.

All routes below need membership of the workspace; reading is `operator`, writing is `admin` unless noted.

| Method | Path | Notes |
|---|---|---|
| GET | `/api/v1/workspaces/:ws/agents` | `{ paused_all, me: AgentSummary \| null, categories: [{key, name, agents: AgentSummary[]}] }`. Other members' personal agents are never listed |
| GET | `/api/v1/workspaces/:ws/agent-templates` | `{ items: [{key, category, pack, version, scope, name, avatar, description, mode, max_autonomy, installed, pack_enabled, spec}] }` |
| GET | `/api/v1/workspaces/:ws/agents/:id` | `AgentDetail` (summary + `spec` + `versions`). 404 for another workspace's agent or someone else's personal agent |
| PATCH | `/api/v1/workspaces/:ws/agents/:id` | `{status?: "active"\|"paused"\|"archived", name?, avatar?, color?, autonomy_cap?: 0..3}`. A personal agent's owner may edit it (cap at most the template ceiling); admins cannot. The lead cannot be paused or archived here; 409 for `active` while the agent's pack is off |
| DELETE | `/api/v1/workspaces/:ws/agents/:id` | `{deleted: true}` for custom agents only; **409** for `lead` and `system` agents (pause or tune them instead) |
| POST | `/api/v1/workspaces/:ws/agents` | `{spec: AgentSpec, avatar?, color?}` creates a `custom` agent as `draft` at level 0 (version 1, source `manual`). 400 listing every issue (path and message) when the spec is invalid; 409 if the key is taken |
| POST | `/api/v1/workspaces/:ws/agents/:id/clone` | `{name?}` copies the spec into a new `custom` agent (`cloned_from` set) |
| PUT | `/api/v1/workspaces/:ws/agents/:id/spec` | `{spec, note?}` adds a version (source `tuned` for template agents, else `manual`), activates it and sets `tuned`. A change to tools, permissions, steps or drafting caps the agent at level 1 until it passes the gate |
| GET | `/api/v1/workspaces/:ws/agents/:id/versions` | `{ items: [{version, source, gate_status, note, spec_hash, active, created_at}] }` |
| GET | `/api/v1/workspaces/:ws/agents/:id/versions/:v` | `{version, source, gate_status, note, spec, active, created_at}`; `?diff=prev` adds `diff: [{path, before, after}]` against the previous version |
| POST | `/api/v1/workspaces/:ws/agents/:id/versions/:v/activate` | Rollback or roll forward: copies version `v` as a new version (source `rollback`), activates it and restores its automation pins |
| POST | `/api/v1/workspaces/:ws/agents/pause-all` | `{paused: boolean}` pauses (reason `pause_all`) or restores every agent of the workspace; running cases wait |
| GET | `/api/v1/workspaces/:ws/messages/:id/route` | `{message_id, route: {agent_id, agent_name, agent_version, source, shadow, created_at} \| null}` |
| POST | `/api/v1/workspaces/:ws/messages/:id/route` | operator. `{agent_id}` reassigns the message (source `manual`); audited as `message.route_corrected` |
| GET | `/api/v1/workspaces/:ws/agents/:id/outgoing` | `{ items: [{draft_id, message_id, subject, to, send_at, body_preview}] }`: automatic sends held for the delay |
| GET | `/api/v1/workspaces/:ws/agents/:id/audit` | `{ items: [{at, case_id, message_id, tool, scope, status, turn, shadow, refs}] }`: one entry per tool call the agent made, reads included (`status` is `ok`, `error`, `blocked`, `waiting_approval` or `simulated`; `refs` holds ids only, never mail text) |
| GET | `/api/v1/workspaces/:ws/cases?agent_id=&include_shadow=` | the existing case list, filtered by agent; observing agents' trial cases are left out unless `include_shadow=true`. Cases have `agent_id`, `agent` (badge), `shadow` and `trigger` |

```ts
interface AgentSummary { id; key; kind: 'lead'|'system'|'custom'; scope: 'workspace'|'personal'; owner_user_id: string|null; category;
  name; avatar; color; description; status: 'draft'|'active'|'paused'|'archived'; paused_reason: string|null;
  template_key: string|null; tuned: boolean; update_available: boolean; active_version: number|null;
  autonomy: { earned: Autonomy; cap: Autonomy; effective: Autonomy; limited_by: 'earned'|'admin_cap'|'spec'|'workspace'|'profile'|'paused' };
  today: { routed: number; cases: number; approvals_pending: number; cost_usd: number } }
```

**Scopes.** A spec lists the scopes the agent holds: `read_mail` (read mail, threads, memory, search), `draft`, `send`
(send, submit), `label` (tasks, case notes) and `schedule` (wake later, propose automations). Every tool belongs to one
scope; a tool outside the agent's scopes is rejected when the spec is saved and is never offered to or run for the
agent, so a `read_mail`-only agent cannot draft or send whatever an email says. Each tool call is audited
(`agent.tool_call`), approvals store a hash of what the person approved, and the leak check (an address that is not in the
conversation, or a long number) and the recipient rules are code, not a model's opinion.

**Workflows.** A `mode: "workflow"` agent (the Inbox Assistant and every personal agent) runs a routine email as fixed
steps with no planner call: draft, check, ask or send, then wait for a reply and nudge. The planner takes over only for
a complaint, legal wording, several parties, three or more asks, a reopened conversation, a declined draft, a reply
that wakes the case, or a step that failed or was stopped (`state.wf = "planner"`, `state.wf_why`).

**Planner context and models.** The planner sees understanding fields and short fenced excerpts with words joined by `^`
(never raw bodies; `read_email {excerpt: true}` asks for a longer one), only its agent's tools, and a prompt ordered for
caching (case header, steps, latest details, facts, then the turn and budget line). `max_tokens` is 700. One tier rule:
the agent's quality tier is the floor and a premium case or draft can only raise it to `best`.

Environment: `AGENT_MAX_AUTONOMY` (0 to 3, default 3), `AGENT_AUTO_SEND_DELAY_SECS` (default 600), `AGENT_AUTO_SEND_DAILY_CAP`.

Tenancy: `agents::case::get_in` is the only case lookup and always takes the workspace; job payloads carry it too.
Another workspace's case, approval, agent, version or route id is a 404 on every route above and on the existing case and approval routes.

## Team and staffing operations (WP-G)

Two layers. **Generic** (every workspace): the people directory, the super admin and role-scoped views, layered
escalation, per-person mailbox scoping and the metrics framework with the corner widget. **HR & recruiting pack only**
(`staffing`): billable / non-billable, timesheets, submissions and outcomes, portal reconciliation, matching. HR endpoints
answer `409 conflict` in other workspaces; HR fields are rejected (people) or ignored (settings) there.

### Roles

| Level | Who | Can |
|---|---|---|
| `super_admin` | The workspace owner (row in `workspace_owners`: the creator; existing workspaces backfilled with their first admin) | Everything: directory, targets, settings, imports, scan, every person's numbers |
| `admin` | Other admins | Read-only, scoped like operators |
| `operator` | Members | Read-only: themselves and everyone below them in the manager chain |

The super admin is a separate table, not a new `memberships.role` value, because widening that CHECK would rebuild a
referenced table. `GET /team/access` returns `{level, person, is_manager, report_count, staffing, primary_metric}`.

### Team (any workspace; `/api/v1/workspaces/:ws/team/...`)

| Method | Path | Who | Notes |
|---|---|---|---|
| GET | `/access` | member | Level, own directory entry (matched by email), pack flag |
| GET / PUT | `/settings` | read: member, write: super admin | `tz_offset_minutes`, `manager_after_hours`, `final_after_hours` (null = alert escalate-after), `final_emails`, `below_target_hour`, `weekly_check_hour`; with the pack also `hr_source_id`, `client_domains`, `timesheet_grace_days`, `hours_tolerance` |
| GET | `/people` | member (scoped) | `{items, can_edit}` |
| POST / PATCH / DELETE | `/people`, `/people/:id` | super admin | Fields: `name, emails[], role, manager_id, joining_date (YYYY-MM-DD), daily_target, weekly_target, source_id`; pack: `employee_type (billable\|non_billable), client`. PATCH: missing = keep, `null` = clear. DELETE archives and frees the emails. 400 names the field |
| POST | `/people/import` | super admin | `{csv, dry_run}` → `{dry_run, total_rows, created, updated, skipped, errors:[{row, field, message}]}`. Columns: `name,email` (required; several emails with `;`), `role, manager_email, joining_date, daily_target, weekly_target, mailbox`; pack: `employee_type, client`. Matched by email, so re-imports update. Bad rows are skipped and listed |
| GET | `/metrics/me` | member | `{person, metrics}`; both null when the user is not in the directory (no widget) |
| GET | `/metrics/people/:id` | member (scoped, else 404) | `PersonMetrics` |
| GET | `/metrics/team?manager_id=` | super admin: everyone or one manager's team; others: the people below them | `TeamMetrics` |
| GET | `/escalations` | member (scoped) | Delays, final contacts and each person's chain |

Inbox scoping: `GET /workspaces/:ws/messages?source_id=&person_id=` narrows to one mailbox or to the mailbox linked to a
person (`people.source_id`); both together intersect, a person with no mailbox has no mail, unknown ids are 400, a person
outside the caller's scope is 404.

### Metrics framework

Exact SQL, no model. A `MetricDef { key, label, unit (count|rate|minutes), primary, ratio, sql }` is one query over
`(workspace ?1, person ?2, from ?3, to ?4)`. Every workspace gets `replies_sent` (primary) and `avg_response_minutes`; the
staffing pack adds `submissions` (primary, replacing the generic primary), `interviews`, `selected`, `joined`, `rejected`
and `interview_rate` (interviews / submissions). Daily and weekly targets count the primary metric. Each metric is
computed for today, this week (Monday start) and since the joining date, in the workspace time zone. Team totals sum counts
and recompute ratios from the sums.

### Ask Meera hook (read-only; do not edit `src/assistant/*` from here)

`src/staffing/metrics.rs` is the surface the assistant will call. It does no authorisation: the caller must first apply
`team::access::resolve` / `Access::can_see(person_id)`.

```rust
staffing::metrics::find_people(&db, ws, "priya") -> Vec<PersonRef { id, name, role }>        // resolve a name
staffing::metrics::person_metrics(&db, ws, person_id, now_ms) -> Option<PersonMetrics>       // today / week / since joining
staffing::metrics::team_metrics(&db, ws, Some(manager_id) | None, now_ms) -> TeamMetrics     // sums + ratios
// e.g. "How many submissions did Priya do since joining, and what % got interviews?"
//   metrics.find(|m| m.key == "submissions").since_joining, ...find("interview_rate").since_joining
```

### Staffing (pack; `/api/v1/workspaces/:ws/staffing/...`)

| Method | Path | Who | Notes |
|---|---|---|---|
| POST | `/setup` | super admin | Adds the `submission_update` label (client replies about a candidate) |
| POST | `/scan` | super admin | Runs the submission scan and outcome reader now → `{scanned, created, needs_confirm, outcomes_updated}`. Also runs every 15 min (`staffing_scan` job) |
| GET | `/submissions?person_id=&link_status=&status=` | member (scoped) | Detected submissions with `link_status` (`auto\|needs_confirm\|confirmed\|unlinked`), options, `outcome` (`pending\|interview\|rejected\|selected\|joined`), `reached_interview` |
| PATCH | `/submissions/:id` | the person, anyone above them, super admin | `{candidate_id?, requirement_id?, dismissed?, outcome?}`; links become `confirmed`; a manual outcome is not overwritten by the reader |
| GET | `/timesheets?month=YYYY-MM` | member (scoped) | Billable people: `received\|pending\|missing`, due date (month end + grace days), hours read from the mail |
| POST | `/portal/hours`, `/portal/submissions` | super admin | `{csv, dry_run}`; hours: `email,month,hours`; submissions: `email,date,candidate,client` → same report shape as the people import |
| POST | `/portal/reconcile` | super admin | Compare now → `{months, matched, unverified, mismatches, alerts_raised}` |
| GET | `/portal/mismatches` | member (scoped) | Open and resolved mismatches |

Detection rules: an outbound message from a directory person (sender address, or their linked mailbox) to an external,
non-free-mail, non-internal recipient domain (or a configured `client_domains` entry) with a `pdf/doc/docx/rtf/odt`
attachment that is not named like an invoice, timesheet, offer, JD or requirement. One submission per (person, thread,
candidate name from the file name). Links: candidate by exact name / name tokens in the file name or mail / candidate
email; requirement by client (recipient domain vs requirement client), role words and an existing match; embeddings
only suggest and never link on their own. Outcomes come from the client's replies in the thread, or from replies
labelled `submission_update` that name the candidate, read by plain phrases (interview / rejected / selected / joined).

Billable policy: with `hr_source_id` set, mail that a billable person SENT (sender or, for forwarded mail,
`original_sender_address`), or that sits in a mailbox linked to a billable person, and that did not arrive through the HR
mailbox, is stored with `email_messages.suppressed = 1` (audit `message.suppressed` + a warn log). It is never classified,
indexed, listed, searched or alerted on. Recipients and Cc never match, so a client copying a consultant, a team list and a
recruiter's sent CV mail copying a consultant are all kept. Attachment metadata rows are stored for suppressed mail (nothing is
extracted). After any change of `hr_source_id`, a person's billable flag, emails or mailbox, every suppressed message is judged
again against the policy as it now is; those that no longer qualify are released and get the steps they skipped (contact,
agent wake-up, attachment extraction, classification). Mail that still qualifies stays suppressed. Suppressed mail never counts
as a reply and is a 404 on every message route. Marking anyone billable needs a connected HR mailbox, and `hr_source_id` cannot be unset or point at a
disconnected mailbox while billable people exist. A billable person cannot have their own mailbox linked.

Timesheets belong to a month: a month named in the subject ("September timesheet", "Sept", `2026-09`) wins, otherwise
`received_at` minus the grace days decides (a timesheet sent 2 Sep is August's). Portal hours for a month are only compared
with that month's timesheet.

### Ownership, redaction and the mail wall

The super admin needs the owner row and the `admin` role. The members route refuses to demote them;
`POST /team/ownership {email}` (super admin only, target must be an admin member) hands the role over. Non-owners get
`GET /team/settings` and `/team/escalations` without any email addresses, HR mailbox or sources. With the staffing pack and
team setting `restrict_mail_to_team`, operators read only mailboxes linked to people in their own manager-chain subtree
(inbox list, message detail and search; filters cannot step outside it). The same wall applies to every message action
(classification, draft, reprocess, insight), the `/drafts/{id}` routes, the review, alert and workflow-run lists and a contact's
recent messages. Known gap: assistant and agent tools do not apply the wall yet. Off by default. Team setting
`route_alerts_by_owner` (always on in the staffing pack) opts rule alerts into owner routing.

### Alerts and escalation

A resolved alert whose condition returns is reopened at level 1 when the system closed it (a person's own resolve sticks).
`below_target` is one rolling alert per person (`alerts.period_key` = the window), resolved when the target is met or the
window closes. New kinds: `below_target` (every workspace), `timesheet_missing` and `portal_mismatch` (pack). `alerts.person_id` is the
owner. For an alert with an owner: level 1 goes to the person, level 2 to their manager (else the final contacts), level 3
to the final contacts (else the alert recipients). Delays: `manager_after_hours` then `final_after_hours`. Rule alerts
(`important_unanswered`, `approval_pending`) get the owner of the mailbox they arrived in.

## Email understanding v2 (`UNDERSTAND_V2`)

`off` (default) keeps classify + analyze + judges. `shadow` keeps them in charge and also runs `ai::understand` once per
email, recording label agreement. `on` lets the one call decide (label, priority, needs_reply, asks, commitments, deadlines,
entities, facts, alert signals); `classifications` and `message_insights` are written exactly as before, plus
`message_insights.understanding_json`. Asks, commitments, deadlines and entities whose quote is not in the email are dropped;
`*_iso` dates outside received -30/+365 days are nulled; a deadline within 2 days forces `priority = high`. Any failure falls
back to the separate calls.

| Method | Path | Role | Response |
|---|---|---|---|
| GET | `/api/v1/workspaces/:ws/understanding-shadow` | admin | `{compared, failed, label_agreement, needs_reply_agreement, priority_agreement, min_agreement: 0.95, min_samples: 100, ready_to_switch}` |

Bulk mail (two or more of List-Unsubscribe, Precedence bulk/list, Auto-Submitted, Gmail promotions/social/updates/forums, or a
no-reply sender; unknown sender; not billing, legal or security content) is filed by rules with no model call
(`classifications.model_id = 'bulk-rules-v1'`, `source = 'fallback'`). Opening it (`POST …/messages/:id/insight`) runs the full pipeline.
