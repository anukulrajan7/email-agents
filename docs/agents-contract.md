# Agentic system: shared contract (build now)

Status: 14 Oct 2026 · Goal: agents that finish work (cases), not a fixed pipeline. Product rules unchanged: AI always on,
never show model names, people approve risky steps, cheap by default.

## Pieces and owners

| Piece | Owner | Files |
|---|---|---|
| AI call ledger + daily budget + `/ai-usage` + usage card | Agent L | `src/ai_ledger.rs`, migration `0021_ai_ledger.sql`, `http/ai_usage_api.rs`, hook in `ai/openrouter.rs::chat_json`, `apps/web/src/features/usage/` |
| Agent runtime + tools + planner + critic + approvals + Requirement agent + Inbox agent | Agent R | `src/agents/*` (new), migration `0022_agents.sql`, `http/agents_api.rs`, `jobs/agent_jobs.rs` |
| Agents UI: cases, timeline, approvals inbox, nav badge | Agent U | `apps/web/src/features/agents/` (new), route + nav |

Shared files (`http/mod.rs`, `jobs/mod.rs`, `jobs/handlers.rs`, `lib.rs`, `config.rs`, `routes.tsx`, `AppShell.tsx`): small additive edits only.

## Runtime model

- **Case** = one unit of work with a goal, state, budget and owner. Kinds: `requirement` (fill a client requirement),
  `inbox` (handle one new email end to end). Status: `running` → (`waiting_approval` | `waiting_event`) → `done` | `failed` | `cancelled`.
- **Step** = one planner turn: the planner (LLM with tool calling) reads the goal, case state and previous steps and
  returns either one tool call or `finish {outcome, summary}`. Each step is one durable job (`agent_step:<case>:<seq>`),
  so cases survive restarts and can sleep for days.
- **Tools** have a JSON schema and a risk: `read` and `low` run immediately; `high` (send_email, submit_candidate)
  creates an **approval** and the case waits. Before any `low`/`high` tool runs, a **critic** (cheap model) checks the
  proposal against the case facts and rules; a failed check becomes a note the planner sees next step.
- **Limits**: `max_steps` (default 15), `budget_usd` (default 0.10 per case). Hitting either → `failed` with reason.
- **Wake-ups**: `schedule_check {hours}` sleeps the case; a reply in a linked thread wakes it early.
- **Models**: planner `OPENROUTER_PLANNER_MODEL_ID` (default `anthropic/claude-haiku-5.5`), premium planner
  `OPENROUTER_PREMIUM_MODEL_ID` for `requirement` cases of high priority; critic = classifier model. Test double: a
  scripted planner that replays a list of tool calls.

## Tools (v1)

| Tool | Risk | Input | Does |
|---|---|---|---|
| `search_mail` | read | `{query, limit?}` | hybrid search (`knowledge::search`) |
| `read_email` | read | `{message_id}` | cleaned text + insight |
| `read_thread` | read | `{message_id}` | all messages in the thread, oldest first |
| `find_candidates` | read | `{requirement_id, limit?}` | matches with score + reasons |
| `recall_memory` | read | `{about}` | memories for a contact/client |
| `draft_email` | low | `{message_id, instructions?, quality?}` | draft reply via the tiered drafter; returns draft_id + body |
| `create_task` | low | `{title, message_id?}` | review item for a person |
| `schedule_check` | low | `{hours, reason}` | sleep the case |
| `update_case` | low | `{status_note, outcome?}` | record progress; outcome ∈ submitted, interview, rejected, placed, closed |
| `send_email` | high | `{draft_id}` | approval → `replies::send_draft` |
| `submit_candidate` | high | `{requirement_id, candidate_ids[], draft_id}` | approval → send submission draft, mark matches `shortlisted` |

## Agents (v1)

- **Requirement agent**: starts automatically when a `client_requirement` record is created (after matching runs),
  or by `POST /cases`. Goal: "Shortlist up to 3 candidates for <role> at <client>, get approval, submit, follow up
  until an outcome". Expected path: find_candidates → recall_memory → draft_email (submission, premium) →
  submit_candidate (approval) → schedule_check 48 h → read_thread → update_case / draft follow-up.
- **Inbox agent**: starts for a new inbound email that needs a reply and is not `other` (when the workspace's
  auto-reply is on, or when a person clicks "Let the agent handle it"). Goal: "Resolve this email": read →
  recall_memory → draft_email → send_email (approval) or create_task → finish.

## API (all under `/api/v1/workspaces/{ws}`; Operator reads/decides, Admin cancels)

```
GET  /cases?status=&kind=          → { items: CaseSummary[] }
GET  /cases/{id}                   → CaseDetail
POST /cases                        → CaseSummary   body { kind: 'requirement', requirement_id } | { kind: 'inbox', message_id }
POST /cases/{id}/cancel            → CaseSummary
GET  /approvals?status=pending     → { items: Approval[], pending_count: number }
POST /approvals/{id}               → Approval      body { decision: 'approve' | 'decline', edited_body?: string, note?: string }
GET  /ai-usage?days=30             → AiUsage
```
```ts
type CaseStatus = 'running' | 'waiting_approval' | 'waiting_event' | 'done' | 'failed' | 'cancelled';
interface CaseSummary { id: string; kind: 'requirement' | 'inbox'; title: string; goal: string; status: CaseStatus;
  outcome: string | null; steps: number; max_steps: number; spent_usd: number; budget_usd: number;
  message_id: string | null; requirement_id: string | null; next_wake_at: string | null; created_at: string; updated_at: string }
interface Step { seq: number; kind: 'tool' | 'critic' | 'approval' | 'note' | 'finish'; tool: string | null;
  summary: string;            // one plain sentence: "Found 3 candidates (best 93)"
  input: unknown; output: unknown; status: 'ok' | 'error' | 'blocked' | 'waiting'; cost_usd: number; created_at: string }
interface CaseDetail extends CaseSummary { state: Record<string, unknown>; steps: Step[]; approvals: Approval[] }
interface Approval { id: string; case_id: string; case_title: string; action: 'send_email' | 'submit_candidate';
  summary: string;            // "Send submission of 3 candidates to hr@capgemini.com"
  to: string | null; subject: string | null; body: string; candidates: { id: string; name: string; score: number }[];
  status: 'pending' | 'approved' | 'declined'; decided_by: string | null; decided_at: string | null; created_at: string }
interface AiUsage { days: number; total_usd: number; today_usd: number; budget_usd_per_day: number;
  by_day: { date: string; usd: number; calls: number }[];
  by_task: { task: string; usd: number; calls: number }[];          // task names, not model names
  per_email_usd: number }                                           // average automatic cost per new email
```
UI never shows model names; `by_task` uses task names (Labelling, Summaries, Drafts, Memory, Agents, Search).
