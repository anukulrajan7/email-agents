# 08 — Personal Learning Roadmap

**Summary.** You know React, Next, TypeScript, Node and MongoDB. This system adds Rust, SQLite, a hand-built job queue, an AI pipeline and an agent runtime, and it was written in two days by agents. The goal of this roadmap is that you can rebuild, change and defend every important decision in it. The syllabus is ordered by what the audit found you most need, each lesson is tied to a file and a real defect, and each ends with an exercise and three questions. It complements the existing `docs/learn/` modules (00 to 05 exist; 06 to 08 are planned there), which are broader; this roadmap is narrower and tied to the fixes in `07`.

Rule for using it: one lesson per sitting. Do the exercise before reading the next. Write your answers down. When you ask for a lesson in a session, say its number; I will teach that one topic, check your answers, and only then continue.

---

## Track A — Rust and the backend (lessons 1 to 7)

### Lesson 1 — Ownership, `Arc`, and why `tokio::spawn` wants owned values
- **Why it matters:** every job and request clones `AppState`; misunderstanding this is the first wall in Rust.
- **Example:** `apps/api/src/jobs/worker.rs:35-43`; `http/mod.rs:59-102` (27 `Arc` fields).
- **Docs:** Rust Book ch. 4 (ownership), ch. 15.3 (`Rc`/`Arc`), Tokio tutorial "Spawning".
- **Exercise:** remove `let state = state.clone();` in `worker.rs`, read the compiler error, fix it two different ways (clone outside; pass by `Arc` explicitly).
- **Questions:** Why is cloning `AppState` cheap? What would change if a field were `Vec<u8>` instead of `Arc<…>`? Why does `async move` need `'static` data?
- **Common mistakes:** cloning inside the loop body after the move; fighting the borrow checker with `Rc` in async code (not `Send`).

### Lesson 2 — `Result`, `?`, `From`, `thiserror`, `anyhow`
- **Why:** error handling is the control flow of this backend; `AppError` decides every HTTP status.
- **Example:** `error.rs:37-121`; `jobs/handlers.rs::JobError`; `ai/mod.rs:179` `AiError::{Retryable, Permanent}`.
- **Docs:** Rust Book ch. 9; `thiserror` and `anyhow` READMEs.
- **Exercise:** `03` exercise 2 (add `AppError::Gone` → 410 with a test).
- **Questions:** When does `?` call `From`? Why does `AppError::Internal` hide its message from clients? Why are `Retryable` and `Permanent` separate variants instead of a boolean?
- **Mistakes:** `unwrap()` in request paths; mapping every error to 500; `anyhow` in library APIs where callers need to match.

### Lesson 3 — Traits, generics and trait objects (the AI ports)
- **Why:** `Arc<dyn Classifier>` is how tests swap models for doubles and how a missing key becomes a rules fallback.
- **Example:** `ai/mod.rs:187-202`; `lib.rs:94-109`; `tests/common/mod.rs` `BodyDrafter`.
- **Docs:** Rust Book ch. 10 and ch. 18.2 (trait objects); `async-trait` README.
- **Exercise:** write a `CountingClassifier` wrapper that implements `Classifier` by delegating and counting calls; use it in one test to assert "one classify per email".
- **Questions:** Why `Send + Sync` on the trait? Why does `async fn` in a trait need `#[async_trait]` here? When would a generic `C: Classifier` beat `dyn Classifier`?
- **Mistakes:** returning `impl Trait` where a trait object is needed; forgetting `Arc` and cloning a boxed trait.

### Lesson 4 — async/await, tokio tasks, `select!`, semaphores, blocking
- **Why:** the worker, the shutdown and the Argon2 limiter are all tokio primitives; C10 (capacity) is an async question.
- **Example:** `jobs/worker.rs:14-63`; `sessions.rs:49-72`; `main.rs:51-65`.
- **Docs:** Tokio tutorial (Spawning, Shared state, Select); "Async in depth".
- **Exercise:** `03` exercise 5; then add a 30 s heartbeat task to `process` that extends the lease and is aborted on completion (this is fix P1.1; write the test first).
- **Questions:** What does a task yield on? Why is `spawn_blocking` used for Argon2 but not for SQL? What happens to in-flight jobs at SIGTERM?
- **Mistakes:** blocking the runtime with `std::fs` or CPU work; holding a `std::Mutex` guard across `.await`; forgetting the `JoinHandle` (C2).

### Lesson 5 — axum from first principles: Router, extractors, middleware, state
- **Why:** you asked for this specifically; it is also fix P2.3 (`Member` extractor).
- **Example:** `http/mod.rs:104-222`; `auth.rs:189-196`; `security.rs:63-72`; `error.rs:21-35`.
- **Docs:** axum docs: `Router`, `extract`, `middleware::from_fn`, "Sharing state with handlers".
- **Exercise:** `03` exercises 3 and 6 (a `Member` extractor; a new module with `routes()`; make the isolation test fail then pass).
- **Questions:** In what order do layers run, and why is it the reverse of reading order? Why must the body extractor be last? What does `.with_state` change about the router's type?
- **Mistakes:** putting authorisation in a layer that cannot see path params; `nest` where `merge` is meant; forgetting `route_layer` versus `layer`.

### Lesson 6 — SQLite for a backend: WAL, locks, `BEGIN IMMEDIATE`, transactions in sqlx
- **Why:** the single most important operational rule in the repo; also idempotency keys.
- **Example:** `db.rs:28-50`; `jobs/handlers.rs:528-640`; `jobs/mod.rs:118-149`; `tests/db_concurrency.rs`.
- **Docs:** SQLite "Write-Ahead Logging", "Transaction", "Isolation"; sqlx `Transaction` docs.
- **Exercise:** `03` exercise 4 (a `ping` job enqueued twice; one audit row). Then change one `begin_with("BEGIN IMMEDIATE")` to `.begin()` and run `tests/db_concurrency.rs`. Predict first.
- **Questions:** Why can a read-then-write transaction fail where an immediate one waits? What does `ON CONFLICT … DO UPDATE … WHERE status='dead'` achieve? Why must no network call happen inside a transaction?
- **Mistakes:** long transactions; relying on `busy_timeout` for upgrades; `UNIQUE` without a planned conflict action.

### Lesson 7 — Durable jobs: leases, fencing, backoff, dead letters, idempotency
- **Why:** C1, C2, C5 all live here; this is the backbone of the product.
- **Example:** `jobs/mod.rs:186-265`; `replies.rs:287` (`reconcile`); `README.md` "Key design choices".
- **Docs:** Tokio `Semaphore`; "Designing Data-Intensive Applications" ch. 11 (exactly-once is a myth); any writeup on "fencing tokens".
- **Exercise:** write the C1 heartbeat test (sleep longer than a 1 s lease; assert one run); then implement the heartbeat.
- **Questions:** Which sequence of events does `WHERE attempts = ?` in `complete` prevent? Why is `send_attempt_id` used as the Message-ID? What makes a handler idempotent, in one sentence?
- **Mistakes:** retrying side effects without a key; treating `succeeded` as "work done" (deferred jobs, `handlers.rs:119`).

---

## Track B — TypeScript and React (lessons 8 to 11)

### Lesson 8 — Promises and async control flow, with `api()` and mutation chains
- **Example:** `apps/web/src/api/client.ts:59-83`; `features/drafts/hooks.ts` `useSendDraft`.
- **Docs:** MDN "Using promises", "async function".
- **Exercise:** `04` exercise 1. **Questions:** `04` §11 Q1. **Mistakes:** unhandled rejections in `onSuccess`; awaiting in a loop when `Promise.all` is meant.

### Lesson 9 — The trust boundary: `unknown`, narrowing, runtime validation
- **Example:** `api/client.ts:24-28` `isErrorBody`; `api/types.ts`.
- **Docs:** TS handbook "Narrowing"; zod README; `openapi-typescript`.
- **Exercise:** `04` exercise 2; then decide (in writing) between generated types and zod for this repo. **Questions:** `04` §11 Q3. **Mistakes:** `as T` as a habit; validating in components instead of at the boundary.

### Lesson 10 — React rendering: keys, state, effects, closures
- **Example:** `features/inbox/InboxPage.tsx:37`; `features/drafts/DraftPanel.tsx` (`DraftEditor` key, `UndoBar` timer, `onAwaitingChange` effect); `AgentsHubPage.tsx:45`.
- **Docs:** react.dev "Preserving and Resetting State", "You Might Not Need an Effect", "Synchronizing with Effects".
- **Exercise:** `04` exercises 3 and 4. **Questions:** `04` §11 Q2; what breaks under StrictMode's double render in `AgentsHubPage`? **Mistakes:** state synced via effects; stale closures in timers; index keys on reorderable lists.

### Lesson 11 — Server state with TanStack Query; error boundaries
- **Example:** `app/providers.tsx`; `features/inbox/hooks.ts`; `features/agent-builder/hooks.ts` (`setQueryData`).
- **Docs:** TanStack Query "Important Defaults", "Query Invalidation", "Optimistic Updates"; react.dev "Error Boundary".
- **Exercise:** `04` exercises 5 and 6 (optimistic archive; error boundary with a test). **Questions:** Why structural key hashing? When is `invalidateQueries(['ws', ws])` too broad? Why does an error boundary not catch errors in event handlers? **Mistakes:** fixed-interval polling everywhere; one QueryClient per render.

---

## Track C — AI engineering (lessons 12 to 17)

### Lesson 12 — Tokens, context windows and budgets in code
- **Example:** `memory.rs:208-266`; `agents/runner.rs:28-94`; `docs/ai-engineering-review.md` §4.
- **Docs:** OpenRouter "Usage accounting"; Anthropic "Effective context engineering".
- **Exercise:** compute the planner prompt size for a 10-step case by hand from the caps; then read `ai_calls.tokens_in` for a real case and compare. **Questions:** Which block is dropped first when over budget? Why does the email go last? What is the cost difference between 1,000 cached and uncached input tokens on V4 Flash (`06` §1.1)?

### Lesson 13 — Structured outputs and verification
- **Example:** `ai/openrouter.rs:197-277` (`strict`, `reasoning: false`, `validate` closure); `ai/understand.rs:233-357` (quote check).
- **Docs:** OpenRouter "Structured outputs"; JSON Schema basics.
- **Exercise:** add a required field to the understand schema and watch what fails; then write a code check for it. **Questions:** Why is "copy, never infer" enforced in code and not in the prompt? Why are schema failures `Retryable` today and why is that wrong (H10)? What does `require_parameters` do?

### Lesson 14 — Tool calling and the agent loop as a state machine
- **Example:** `agents/tools.rs:176-197`; `agents/runner.rs:146-275`; `agents/planner.rs`.
- **Docs:** OpenRouter "Tool calling"; Anthropic "Building effective agents".
- **Exercise:** add a read-only tool `count_unanswered_asks` with a JSON schema, register it, allow it in one template, and write an offline golden scenario that uses it. **Questions:** Where is the fence that stops a turn from being recorded twice? What are the three ways a case ends? Why does the critic run before low-risk tools but not before read tools?

### Lesson 15 — RAG and memory without a vector database
- **Example:** `knowledge.rs` (FTS5 + sqlite-vec, 800-char chunks, 1024-dim bge-m3); `memory.rs`; `learning/retrieve.rs`.
- **Docs:** SQLite FTS5; sqlite-vec README; the bge-m3 model card.
- **Exercise:** run a hybrid search by hand with `sqlite3` on a dev DB; explain why FTS and vector results are merged. **Questions:** Why is memory capped at 2,500 bytes? What stops a fact learned from outside mail from being trusted? What is the cost of embedding one email?

### Lesson 16 — Model routing, evals and the promotion gate
- **Example:** `ai/openrouter.rs:633` (`ESCALATE_BELOW`), `agent_profiles::tier_model`; `evals/safety.rs:526` (`score`); `evals/gate.rs:88` (`requirements`).
- **Docs:** Anthropic "Demystifying evals for AI agents".
- **Exercise:** run `cargo test --test golden_offline -- --nocapture`; add one scenario (`inj_subject_line`, from `07` C4); make it fail, then fix the fence. **Questions:** What do offline evals prove and what can they not prove? What exactly must be true to reach level 2? Why is demotion automatic but promotion manual?

### Lesson 17 — Agent security: injection, the lethal trifecta, approvals
- **Example:** `agents/injection.rs`; `ai/fence.rs`; `send_safety.rs:236, 501`; `agents/approvals.rs:177`; `docs/research-2026-10.md` "Security".
- **Docs:** Simon Willison on prompt injection and the lethal trifecta; CaMeL paper; Spotlighting paper.
- **Exercise:** `07` C4 fix (fence subject and sender in the planner and critic prompts) with a golden test. **Questions:** Why is a model-only classifier not a defence? Why must recipients be fixed to thread participants? What does the content hash on an approval protect against?

---

## Track D — Product economics (lesson 18)

### Lesson 18 — Unit economics from the ledger
- **Example:** `ai_ledger.rs:56-71`, `usage`; `06-cost-optimization.md` §2 to §4.
- **Exercise:** reproduce the ₹0.113 per email figure from the ledger of a real day; then compute what P2.1 and P2.2 would save for your actual volume. **Questions:** Which single variable moves the bill most? At what mailbox count does the worker, not money, become the limit? Why price per mailbox and not per credit?

---

## Suggested order and pace

| Week | Lessons | Ships |
|---|---|---|
| 1 | 1, 2, 5 | `Member` extractor on one module (P2.3 start) |
| 2 | 6, 7, 4 | Heartbeat + panic capture (P1.1, P1.2) |
| 3 | 8, 9, 10, 11 | Error boundary + admin guard + one validated response (P1.6, P2.4 start) |
| 4 | 12, 13, 17 | Fencing fix and `injection_signals` (P1.4) |
| 5 | 14, 16 | Workflow-first inbox (P2.2) with a new golden scenario |
| 6 | 15, 18, 3 | Budget choke point (P1.3) |

Each week pairs lessons with a Phase 1 or 2 task from `07`, so learning and stabilising are the same work.

---

## First lesson

Lesson 5 (axum extractors) is the best first high-value lesson: it answers your explicit question about routers, it leads directly to fix P2.3, and it needs only lessons 1 and 2 as background. The chat message that accompanies this document starts it.
