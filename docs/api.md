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
  `not_implemented` (501), `email_not_verified` (403), `rate_limited` (429), `internal` (500).

## Enums

```text
Role              = "admin" | "operator"
SourceKind        = "forwarding" | "nango_gmail" | "nango_microsoft" | "future"
SourceStatus      = "pending" | "connected" | "degraded" | "disconnected"
ProcessingStatus  = "pending" | "normalized" | "classified" | "workflow_completed" | "failed"
Category          = "timesheet" | "leave" | "candidate_referral" | "approval" | "payroll" | "general" | "other"
Priority          = "low" | "normal" | "high"
ClassSource       = "jev" | "llm" | "manual" | "fallback"
DraftStatus       = "suggested" | "edited" | "approved" | "sent"
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
| POST | `/api/v1/workspaces` | any | `{name}` | `Workspace` (caller becomes admin; 3 workflow templates seeded) |
| POST | `/api/v1/workspaces/:ws/members` | admin | `{email, role}` | `{user_id, email, role}` |
| GET | `/api/v1/workspaces/:ws/sources` | member | – | `{items: Source[]}` |
| POST | `/api/v1/workspaces/:ws/sources/forwarding` | admin | – | `Source` (idempotent: returns existing forwarding source) |
| POST | `/api/v1/workspaces/:ws/sources/:id/test-email` | admin | `{from?, subject?, body_text?}` | `{status:"queued"}` (only if `ALLOW_TEST_EMAILS=true`, else 404) |
| POST | `/api/v1/workspaces/:ws/sources/nango-session` | admin | `{provider: "gmail"\|"microsoft"}` | `{token, expires_at, provider_config_key}`; 501 if Nango unconfigured or provider is microsoft |
| POST | `/api/v1/workspaces/:ws/sources/nango-connections` | admin | `{connection_id, provider_config_key}` | `Source` (server re-verifies connection tags with Nango; 403 if not this workspace/user) |
| POST | `/api/v1/workspaces/:ws/sources/:id/sync` | admin | – | 202 `{status:"queued"}`; 409 for forwarding or disconnected sources |
| POST | `/api/v1/workspaces/:ws/sources/:id/disconnect` | admin | – | `Source` (status `disconnected`; Nango connection revoked best-effort) |
| GET | `/api/v1/workspaces/:ws/messages?cursor=&category=&q=&limit=` | member | – | `{items: MessageSummary[], next_cursor: string\|null}` |
| GET | `/api/v1/workspaces/:ws/messages/:id` | member | – | `MessageDetail` |
| PATCH | `/api/v1/workspaces/:ws/messages/:id/classification` | member | `{category, priority?, requires_reply?}` | `Classification` |
| POST | `/api/v1/workspaces/:ws/messages/:id/draft` | member | – | 202 `{status:"queued"}` |
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

type Draft = { id: string; message_id: string; status: DraftStatus; body_text: string; model_id: string | null; created_at: string; updated_at: string };

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

## Auth: email + password (v1)

`AUTH_MODE=password` (production). `AUTH_MODE=dev` (local only) keeps the `dev:<email>` token. The client always
sends its token as `X-Auth-Token`. Sessions are opaque random tokens (only a SHA-256 hash is stored), 30-day expiry.

| Method | Path | Auth | Body | Response |
|---|---|---|---|---|
| GET | `/api/v1/auth/config` | – | – | `{mode: "password"\|"dev"\|"oidc", signup_enabled: boolean}` |
| POST | `/api/v1/auth/signup` | – | `{email, password, name?}` | 201 `{token, expires_at, user: {id, email, name}}` |
| POST | `/api/v1/auth/login` | – | `{email, password}` | `{token, expires_at, user: {id, email, name}}` |
| POST | `/api/v1/auth/logout` | token | – | 204 (revokes this session) |

Rules and errors:
- Password 10–128 characters. Email normalized to lowercase. `name` optional, ≤ 100 characters.
- Signup: 409 `conflict` if the email already has an account; 403 `forbidden` if signup is disabled or the
  email domain is not in `SIGNUP_ALLOWED_DOMAINS` (when set).
- Login: always the same 401 `unauthorized` "invalid email or password" (no account enumeration);
  429 `rate_limited` after too many failed attempts for that email (15-minute window).
- `GET /api/v1/me` returns `user.name` too, and `auth_mode`.

## Email verification (OTP)

Enabled when SMTP is configured (`SMTP_HOST`, `SMTP_PORT`, `SMTP_USERNAME`, `SMTP_PASSWORD`, `SMTP_FROM`).
Codes are 6 digits, valid 10 minutes, max 5 attempts, stored only as a hash.

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/v1/auth/signup` | as before | **202** `{verification_required: true, email}` when verification is on (no token yet); 201 session otherwise |
| POST | `/api/v1/auth/verify` | `{email, code}` | `{token, expires_at, user}` (signs the user in); 400 `bad_request` "invalid or expired code"; 429 `rate_limited` |
| POST | `/api/v1/auth/resend-code` | `{email}` | 202 `{status:"sent"}` always (no account enumeration; at most one code per 60 s) |
| POST | `/api/v1/auth/login` | as before | **403 `email_not_verified`** if the password is right but the email is unverified (a fresh code is sent) |

`GET /api/v1/auth/config` gains `email_verification: boolean`.

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
| POST | `/api/v1/workspaces` | any | `{name, template?: LabelTemplate}` (default `"hr"`) | `Workspace` |
| GET | `/api/v1/workspaces/:ws/labels?include_archived=` | member | – | `{items: Label[]}` ordered by `position` |
| POST | `/api/v1/workspaces/:ws/labels` | admin | `{name, description, color?}` | `Label` (key = slug of name; 409 if taken) |
| PATCH | `/api/v1/workspaces/:ws/labels/:id` | admin | `{name?, description?, color?, archived?, position?}` | `Label` (409 when archiving `other` or a label used by an enabled workflow) |

Rules: `name` 1–40 chars, `description` 10–300 chars, max 30 active labels per workspace.
`PATCH .../messages/:id/classification` and `GET .../messages?category=` validate against the workspace's active labels.
Workflow templates attach to a label key (`timesheet_review` → `timesheet`); `Workflow` gains `label_key: string`
and `available` is false when the workspace has no such active label.

## Matching: referrals ↔ client requirements (P1)

Emails labelled `client_requirement` become **Requirements**; emails labelled `candidate_referral` (and their
PDF/DOCX CVs) become **Candidates**. Each new record is scored against every open counterpart (0–100: must-have
skills 60, experience 25, location 15) and kept when the score is ≥ 40, with reasons. The AI proposes; a recruiter
shortlists or dismisses. Re-scoring never overrides a recruiter's decision.

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
Manual `POST /drafts/{id}/send` is also refused with 409 `draft_incomplete` while the body contains `[NEEDS:`.

`remind_if_no_reply` schedules a delayed job. When due, if the draft was not sent and no later message from someone else
arrived in the thread, a review item of kind `reminder` ("No reply yet: ...") is opened.
