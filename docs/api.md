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
  `not_implemented` (501), `internal` (500).

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
