# L5 — Glossary

Every term used in this codebase and these docs, in plain words, with where it lives. Paths are under `apps/api/src/` unless they start with `apps/web`.

## Rust language and tooling

| Term | Plain meaning | Where in our code |
|---|---|---|
| Cargo | Rust's build tool and package manager (npm + tsc + jest) | `apps/api/Cargo.toml` |
| Crate | One compiled package; ours is `email-ai-api` | `lib.rs`, `main.rs` |
| Module | A namespace inside a crate, declared with `mod` | `lib.rs` lists 52 |
| Edition | Rust language version set; ours is 2024 | `Cargo.toml` |
| `let` / `let mut` | Immutable / mutable variable | everywhere |
| `String` / `&str` | Owned text / borrowed view of text | `db::new_id` |
| `Vec<T>` / slice `&[T]` | Growable array / borrowed view of one | `routing::route` |
| Struct | Named group of fields (a typed object) | `jobs::Job` |
| Enum | A value that is one of several variants, each with its own data | `agent_registry::spec::Step` |
| `match` | Branch on every variant; the compiler checks none is missed | `error.rs:87` |
| `Option<T>` | `Some(value)` or `None`; Rust has no null | `Job.workspace_id` |
| `Result<T, E>` | `Ok(value)` or `Err(error)`; errors are values | `AppResult<T>` |
| `?` operator | Return the error now, else unwrap the value | every handler |
| `unwrap` / `expect` | Take the value or crash; fine in tests only | flagged in `sessions.rs` |
| Ownership | Every value has one owner; freed when the owner goes away | Rust Book ch. 4 |
| Move | Ownership passes to a new variable; the old one is unusable | `tokio::spawn(async move ...)` |
| Borrow (`&`, `&mut`) | Temporary access without taking ownership; many readers or one writer | `route(&facts, ...)` |
| Lifetime (`'a`) | How long a borrow is valid | `SendFacts<'a>` |
| Clone | Make an explicit copy | `state.clone()` in `jobs/worker.rs` |
| Trait | An interface: a set of methods a type promises | `ai::Classifier` |
| `impl` | Where methods or trait implementations are written | `impl Job` |
| Generic | Code with a type parameter, copied per type at compile time | `impl SqliteExecutor` in `agent_registry/store.rs` |
| Trait object (`dyn Trait`) | A value of "some type implementing the trait", chosen at runtime | `Arc<dyn Classifier>` |
| Static / dynamic dispatch | Method chosen at compile time / at runtime via a vtable | generics / `dyn` |
| `derive` | Ask the compiler to write standard code (Debug, Clone, Serialize) | `#[derive(Debug, Clone, FromRow)]` |
| Macro | Code that writes code (`format!`, `tokio::select!`, `sqlx::migrate!`) | many |
| Closure | Anonymous function that can capture variables (an arrow function) | `routing.rs:177` |
| Iterator | Lazy sequence with `.filter().map().collect()` | `routing.rs:182` |
| `Arc<T>` | Shared pointer, thread-safe reference counting | `AppState` fields |
| `Mutex` / `RwLock` | One writer at a time / many readers or one writer | `sessions.rs`, `auth.rs:39` |
| `LazyLock` / `OnceLock` | Initialise a global once, on first use | regexes in `spec.rs:17` |
| `Send` / `Sync` | Safe to move to / share between threads | `trait Classifier: Send + Sync` |
| `unsafe` | Code the compiler cannot check; must be justified | `db.rs:15-26` only |
| FFI | Calling C code from Rust | sqlite-vec registration |
| RAII | Resource freed automatically when its owner is dropped | transactions, permits |
| Newtype | A struct wrapping one value to give it meaning | proposed `WorkspaceId`, `Secret` |
| `thiserror` / `anyhow` | Typed error enums / easy errors with context | `AppError` / `main.rs` |
| serde | Serialize and deserialize (JSON) | everywhere |
| clippy / rustfmt | Linter / formatter | `make check` |
| `#[cfg(test)]` | Compile only when testing | unit test modules |
| Integration test | A test file in `tests/` compiled as its own program | 60 files |
| Monomorphisation | The compiler copying generic code per concrete type | compile time cost |

## Async, web and database

| Term | Plain meaning | Where |
|---|---|---|
| Future | A lazy description of async work; runs when awaited or spawned | every `async fn` |
| `async` / `.await` | Define / wait for async work without blocking a thread | everywhere |
| Runtime (tokio) | The engine that runs futures on a thread pool | `#[tokio::main]` |
| Task | A future scheduled by the runtime, like a lightweight thread | `tokio::spawn` |
| `spawn_blocking` | Run blocking or CPU-heavy code off the async threads | `sessions.rs:49` |
| `select!` | Wait on several futures, act on the first | `jobs/worker.rs:30` |
| Semaphore | Counter that limits concurrency | job concurrency |
| `Notify` | Wake a sleeping task | `AppState.jobs_notify` |
| `JoinHandle` / `JoinSet` | Handle to a spawned task's result / a group of them | `worker.rs`, `nango.rs` |
| `task_local!` | Value visible to all code inside one task | request id, AI context |
| axum | Our web framework | `http/` |
| Router | Table from method + path to handler | `http/mod.rs:104` |
| Handler | Async function that answers one route | `messages::request_draft` |
| Extractor | Handler parameter axum fills from the request (`State`, `Path`, `Json`, `AuthUser`) | `auth.rs:189` |
| `IntoResponse` | Anything that can become an HTTP response | `AppError` |
| Middleware / layer | Code wrapped around every request | `request_id_middleware` |
| tower | Library axum's middleware is built on | `TraceLayer` |
| `merge` / `nest` | Combine routers at the same prefix / under a prefix | `http/mod.rs` |
| CORS | Browser rule for which origins may call the API | `CorsLayer` |
| sqlx | Async SQL library | `db.rs` |
| Pool | Set of reusable database connections (8 here) | `SqlitePool` |
| SQLite | File-based SQL database | `data/email_ai.db` |
| WAL | Write-ahead log mode: readers do not block the writer | `db.rs:33` |
| Transaction | Group of statements that all apply or none do | `begin_with(...)` |
| `BEGIN IMMEDIATE` | Take the write lock at the start to avoid upgrade failures | 143 places |
| `busy_timeout` | How long to wait for the lock (15 s) | `db.rs:36` |
| `RETURNING` | Get rows back from an `UPDATE`/`INSERT` in one statement | job claim |
| Migration | Versioned SQL file changing the schema | `migrations/` |
| FTS5 | SQLite full-text keyword search | `knowledge_fts`, `memories_fts` |
| sqlite-vec | SQLite extension for vector search | `knowledge_vec` |
| N+1 query | One query per row inside a loop instead of one batch | `messaging/teams.rs:22` |
| Index | Database structure that makes lookups fast | `idx_claim_jobs_priority` |

## Jobs and reliability

| Term | Plain meaning | Where |
|---|---|---|
| Job queue | Table of work for the background worker | `jobs` table |
| Worker | Loop that claims and runs jobs | `jobs/worker.rs` |
| Claim | Atomically take one due job | `jobs::claim` |
| Lease | Time a worker owns a job (120 s) before others may retake it | `LEASE_MS` |
| Heartbeat | Periodically extending a lease while work continues (missing) | audit C1 |
| Fencing | Only the current owner may update the job (`locked_by` + `attempts`) | `jobs::complete` |
| Backoff | Waiting longer after each failure (2 s doubling to 5 min) | `backoff_ms` |
| Jitter | Random extra delay so retries do not collide | `backoff_ms` |
| Dead letter | A job that gave up; kept for inspection and manual retry | `status = 'dead'` |
| At-least-once | Work may run more than once; handlers must be idempotent | `jobs/mod.rs:1` |
| Idempotent | Running twice has the same effect as once | dedupe keys |
| Dedupe key / idempotency key | Unique key that makes a repeat a no-op | `jobs.dedupe_key`, `drafts.origin_key` |
| Reconcile | Check the outside world before retrying (did the email go out?) | `replies::reconcile` |
| Webhook | Provider calls our URL when something happens | `/webhooks/nango`, `/webhooks/resend` |
| HMAC signature | Proof a webhook came from the provider | `sources/nango.rs:283` |
| Replay attack | Resending an old valid request | Nango webhook gap |
| Graceful shutdown | Finish current work before exiting | `main.rs` |

## AI and agents

| Term | Plain meaning | Where |
|---|---|---|
| LLM | Large language model | via OpenRouter |
| OpenRouter | One API in front of many model providers | `ai/openrouter.rs` |
| Token | Unit of text the model reads and writes (~4 characters) | `ai_calls.tokens_in` |
| Context window | Max tokens per call | model property |
| System prompt | Rules for the model | `DRAFT_SYSTEM_PROMPT` |
| Temperature | Randomness of answers | 0.2 / 0.1 |
| Structured output | Reply forced into a JSON Schema | `chat_json` |
| JSON Schema | A description of allowed JSON shape | `designer/schema.rs` |
| Tool calling | Model asks to run a named function with arguments | `chat_tools` |
| Agent | Model in a loop choosing tools until done | `agents/runner.rs` |
| Workflow | Fixed sequence of steps, some using a model | `agents/workflow.rs` |
| Planner | The model call that picks the next tool | `agents/planner.rs` |
| Critic | A cheap model that checks a proposed action | `agents/critic.rs` |
| Judge | A cheap model giving a typed verdict (yes/no, score) | `ai/judge.rs` |
| Case | One unit of agent work with goal, state, budget | `cases` table |
| Turn / step | One planner decision and its result | `case_steps` |
| Escalation | Hand to a stronger model or to the planner when unsure | `ESCALATE_BELOW` |
| Model routing | Choosing which model answers which task | `ai/routing.rs`, tiers |
| Embedding | Text turned into a vector of numbers | `ai/embeddings.rs` |
| Vector search | Find text with similar meaning | sqlite-vec |
| Hybrid search | Keyword + vector results merged | `knowledge.rs` |
| RAG | Retrieve your own data and add it to the prompt | drafts, classify |
| Chunk | A slice of a document for indexing (800 chars) | `knowledge.rs:21` |
| Memory | Stored facts and preferences used in prompts | `memory.rs`, `learning/` |
| Memory poisoning | Attacker tricks the agent into storing a false rule | guarded in `learning/guard.rs` |
| Context pack | The verified set of facts a draft may use | `learning/context_pack.rs` |
| `[NEEDS: …]` gap | Placeholder where the draft lacks a verified fact; blocks sending | `send_safety.rs` |
| Hallucination | Model states something not supported by its input | quote checks |
| Grounding | Tying every claim to a source | context pack |
| Prompt injection | Untrusted text trying to give the model orders | `agents/injection.rs` |
| Lethal trifecta | Private data + untrusted input + a way to send out | design principle |
| Fencing (prompts) | Wrapping untrusted text in escaped tags | `ai/fence.rs` |
| Datamarking / spotlighting | Marking untrusted words so the model treats them as data | assistant |
| Guardrail | A check that blocks unsafe input or output | send checks |
| HITL | Human in the loop: a person approves risky actions | `agent_approvals` |
| Autonomy level | How much an agent may do alone (0 to 3) | `agents/autonomy.rs` |
| Hold | Delay before an automatic send, with Stop | 600 s |
| Promotion gate | Checklist an agent must pass to gain a level | `evals/gate.rs` |
| Demotion | Automatic level drop after an incident | `incidents.rs` |
| Eval | A test of AI behaviour | `evals/` |
| Golden set | Fixed scenarios with expected outcomes | `evals/scenarios.json` |
| Offline / live eval | With scripted fakes / with real models | `golden_offline.rs` / `golden_live.rs` |
| Shadow mode | New component runs beside the old one for comparison | `UNDERSTAND_V2=shadow` |
| Replay | Re-draft past replies and score closeness | `evals/replay.rs` |
| Ledger | One row per model call with tokens and cost | `ai_calls` |
| Incident | Recorded safety or quality event | `ai_incidents` |
| Prompt caching | Provider reuses an identical prompt prefix cheaply | not yet used |
| Designer | Turns a plain-English request into a validated agent spec | `designer/` |
| Agent spec | The typed definition of an agent: triggers, steps, tools, limits | `agent_registry/spec.rs` |
| Dual-LLM / quarantine | One model reads untrusted text, another acts on typed fields only | target design |

## Product words

| Term | Plain meaning | Where |
|---|---|---|
| Workspace | One team's tenant; all data belongs to one | `workspaces` |
| Tenant isolation | One workspace can never see another's data | `tests/tenant_isolation.rs` |
| Member / role | A user in a workspace: admin or operator | `memberships` |
| Pack | A switchable bundle of labels, agents and pages (General, Support, Sales, Finance, Personal, HR) | `packs.rs` |
| Meera | Each member's personal agent; also the lead | `templates.rs` |
| Expert | An agent for one pack (Ishaan support, Kabir sales…) | `templates.rs` |
| Source | A connected mailbox (Gmail via Nango) or forwarding address (Resend) | `email_sources` |
| Bulk filter | Rules that file newsletters without any model call | `bulk.rs` |
| Understanding | The model's reading of an email: label, summary, asks, deadlines | `ai/understand.rs` |
| Insight | Stored understanding shown in the inbox | `message_insights` |
| Approval inbox | Where people approve agent actions | `apps/web/src/features/agents` |
| AI health page | Admin view of cost, incidents and latency | `ai_health.rs` |
