# Wave 2: AI-native inbox, automations, RAG (task board + shared contract)

Status: 14 Oct 2026 · Product rule: **AI is always on.** No "AI off" mode in the UI; rules/template code paths exist only
as test doubles and error fallbacks. Users never see model names.

## Gaps found (product + AI + engineering review)

| # | Gap | Fix | Owner |
|---|---|---|---|
| G1 | Plain-text bodies show raw `<!--[if !mso]>` comments, `<https://…>` wrappers, duplicated blocks | Clean text at ingest + re-clean stored mail; derive text from HTML when the text part is junk | A (backend core) |
| G2 | Older Gmail mail has no HTML part stored → only plain text | Re-fetch full message for Gmail rows with `body_html IS NULL` (one-time job, idempotent) | A |
| G3 | Emails with no workflow show "Workflow done: in progress" forever | Messages with no matching workflow end in `processed`; UI drops the pipeline tracker | A + D |
| G4 | People can't see *what the email asks* | Insight panel (summary, asks, deadline, needs reply) | A (API) + D (UI) |
| G5 | Inbox order is by time only | `sort=importance` (needs reply + high priority + deadline first) | A + D |
| G6 | "Approve label" step is busywork | Labels are automatic; correction = change label (one click), no approve | D |
| G7 | AI can't propose new labels | `POST /labels/suggest` (AI reads recent mail, mostly `other`) | A + E |
| G8 | Replies not configured on first use | Default automations seeded on workspace create + onboarding step | B + E |
| G9 | Users can't build their own "when X, do Y" | Automations: trigger → conditions → actions, presets, run log | B + E |
| G10 | Labels/drafts don't learn from this team's history | Knowledge store indexing, hybrid search, RAG few-shot labels, RAG drafts | C |
| G11 | Model names visible in UI | Remove from every screen | D + E |

Later waves (not in this wave): people directory + alerts/escalation (M5), submissions + performance widget (M6),
portal CSV reconciliation (M7), Ask agent (M8), golden set + accuracy page.

## Shared API contract (all agents code against this exactly)

All under `/api/v1/workspaces/{ws}`. Existing auth/role rules apply (Operator to read/act, Admin to configure).

### A1. Message list: `GET /messages?sort=importance|recent&cursor=…` (default `recent`, UI uses `importance`)
Each summary item **adds**:
```json
{ "priority": "high|normal|low", "needs_reply": true, "summary": "One-line what-they-want or null" }
```
`sort=importance` orders by: needs_reply desc, priority (high>normal>low), received_at desc. Cursor is an opaque string
(offset for importance sort); `next_cursor` as today.

### A2. Message detail: `GET /messages/{id}` adds
```json
"insight": { "summary": "…", "asks": ["…"], "deadline": "Friday 5pm" , "needs_reply": true, "missing_info": ["…"] } | null
```
and `draft` adds `"unanswered": ["…"]` (asks the draft could not answer; each appears in the body as `[NEEDS: …]`).
`model_id` fields stay in the API but the UI never shows them.

### A3. Processing status
New terminal value `processed` = classified and nothing else to do (no workflow/automation matched).
`ProcessingStatus = 'pending'|'normalized'|'classified'|'workflow_running'|'workflow_completed'|'processed'|'failed'`.

### A4. Label suggestions: `POST /labels/suggest` (Admin) →
```json
{ "items": [ { "key": "invoice", "name": "Invoices", "description": "Vendor invoices and payment requests", "color": "#2F6F4E",
               "example_subjects": ["Invoice #123 for September"], "estimated_count": 12 } ] }
```
Max 5, never duplicates an existing label key. Accept = existing `POST /labels`. Backend: send up to 60 recent
subjects+snippets (prefer category `other`) to the drafting model, JSON schema output; on AI error → 503 `ai_unavailable`.

### B. Automations
```
GET    /automations                    → { items: Automation[] }
POST   /automations                    → Automation       (Admin) body = AutomationInput
PATCH  /automations/{id}               → Automation       (Admin) partial AutomationInput
DELETE /automations/{id}               → 204              (Admin)
GET    /automations/presets            → { items: Preset[] }
POST   /automations/presets/{key}      → Automation       (Admin) create from preset
GET    /automations/{id}/runs?limit=50 → { items: AutomationRun[] }
POST   /automations/{id}/test          → { matched: [ {message_id, subject} ] }  dry run on last 50 messages
```
```ts
type Trigger = 'email_received';                       // fires after classification + insight
type Condition =
  | { field: 'label'; op: 'is' | 'is_not'; value: string }            // label key
  | { field: 'priority'; op: 'is'; value: 'high' | 'normal' | 'low' }
  | { field: 'needs_reply'; op: 'is'; value: boolean }
  | { field: 'from'; op: 'contains'; value: string }
  | { field: 'subject'; op: 'contains'; value: string };
type Action =
  | { type: 'draft_reply'; instructions?: string }           // extra guidance for the drafter
  | { type: 'save_to_gmail_drafts' }                          // needs draft_reply before it
  | { type: 'send_reply'; daily_cap: number }                 // guarded: skipped if draft has [NEEDS], cap reached, or no Gmail source
  | { type: 'create_review'; title?: string }
  | { type: 'remind_if_no_reply'; hours: number };          // creates review item 'reminder' if thread still unanswered
interface AutomationInput { name: string; enabled: boolean; conditions: Condition[]; actions: Action[] }   // conditions AND-ed
interface Automation extends AutomationInput { id: string; trigger: Trigger; preset_key: string | null;
  run_count: number; last_run_at: string | null; created_at: string; updated_at: string }
interface Preset { key: string; name: string; description: string; automation: AutomationInput }
interface AutomationRun { id: string; automation_id: string; message_id: string; message_subject: string;
  status: 'completed' | 'skipped' | 'failed'; actions: { type: string; status: 'done'|'skipped'|'failed'; detail: string | null }[];
  created_at: string }
```
Presets: `reply_to_questions` (needs_reply=true → draft_reply + save_to_gmail_drafts), `acknowledge_timesheets`
(label=timesheet → draft_reply + save_to_gmail_drafts), `chase_important` (priority=high, needs_reply → remind 4h),
`auto_ack_referrals` (label=referral → draft_reply + send_reply cap 20). New workspaces get `reply_to_questions` and
`chase_important` enabled by default. Errors: 400 `invalid_automation` for unknown fields/ops/action order.

### C. Knowledge / RAG (internal, plus one endpoint)
`GET /search?q=…&limit=10` → `{ items: [{ kind: 'email'|'attachment', message_id, subject, snippet, score }] }`.
Indexing, RAG labelling and RAG drafting are internal (no UI change besides search, which is wave 3).

## Work split (disjoint files where possible)

| Agent | Scope | Files it owns |
|---|---|---|
| A backend core | G1–G5, G7 | `ingest.rs`, `sources/*`, `http/messages.rs`, `http/dto.rs`, `http/labels_api.rs`, new `text_clean.rs`, migration `0013_*` |
| B automations | G8, G9 | new `automations.rs`, `http/automations_api.rs`, `jobs/automation_jobs.rs`, migration `0014_*` |
| C knowledge/RAG | G10 | new `knowledge.rs`, `http/search_api.rs`, `jobs/knowledge_jobs.rs`, `ai/embeddings.rs`, migration `0015_*` |
| D inbox UI | G3–G6, G11 (inbox, drafts, reviews) | `apps/web/src/features/{inbox,drafts,reviews}` |
| E builder UI | G7–G9, G11 (rest) | `apps/web/src/features/{automations,labels,onboarding,workflows}`, route + nav entry |

Shared files (`http/mod.rs` router, `jobs/mod.rs` kinds, `jobs/handlers.rs` hooks, `lib.rs`, `api/types.ts`,
`app/routes.tsx`, `AppShell.tsx`): touch with **small additive edits only**; the integrator merges.

## Added: self-learning memory + email alerts (agents F backend, G UI)

| # | Gap | Fix |
|---|---|---|
| G12 | Memory is mostly typed by people | Memory **learns itself**: facts about contacts, clients, standing answers and team preferences are extracted from incoming email + sent/approved replies, deduped, confidence-scored, with source links; people only pin/delete |
| G13 | Nobody is told when something important is missed | **Alerts engine** + outbound **email alerts** (Hostinger SMTP via `mailer.rs`, later Wokay app) with escalation |

### F1. Memory (existing `/memories` endpoints keep working) adds fields
`origin: 'auto'|'manual'`, `confidence: number`, `source_message_id: string|null`, `pinned: boolean`, `use_count: number`,
`last_used_at: string|null`. `GET /memories?origin=auto|manual`. `PATCH /memories/{id}` accepts `{pinned}`.
Auto memories with confidence < 0.6 are kept but never used in drafts until confirmed (`PATCH {confirmed:true}` sets confidence 1).

### F2. Alerts
```
GET   /alerts?status=open|resolved|all     → { items: Alert[], open_count: number }
PATCH /alerts/{id}  { status: 'resolved' | 'open', snooze_hours?: number } → Alert
GET   /alert-settings → AlertSettings      PUT /alert-settings (Admin) → AlertSettings
POST  /alert-settings/test (Admin) → { sent_to: string[] }   // sends a test alert email
```
```ts
interface Alert { id: string; kind: 'important_unanswered'|'approval_pending'|'automation'; severity: 'high'|'normal';
  title: string; body: string; message_id: string|null; message_subject: string|null;
  status: 'open'|'snoozed'|'resolved'; escalation_level: number; notified: string[]; created_at: string; resolved_at: string|null }
interface AlertSettings { enabled: boolean; recipients: string[]; escalate_to: string[];
  unanswered_hours: number; escalate_after_hours: number; daily_digest: boolean; digest_hour: number }
```
Default: enabled, recipients = workspace admins' emails, unanswered_hours 4, escalate_after_hours 24, digest at 9.
