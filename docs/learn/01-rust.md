# 01. Rust through this codebase

**Goals.** Read any file in `apps/api/src` and know what it does. Write a small Axum handler with a SQLx query and
a test, and get it past `cargo clippy -D warnings`.

**Prerequisites.** Comfortable with TypeScript and Node. Rust toolchain installed (`rustup`). Module 00.
**Reading time.** 3 hours, with the exercises. Do it over two or three evenings.

This module does not teach all of Rust. It teaches the ten ideas you meet on every page of this codebase, each
next to a real line of code and next to the TypeScript idea you already know. For the full language, see the Rust
Book in [08. Resources](08-resources.md).

## 1. Why Rust here

The API is Rust (Axum + SQLx + Tokio) for three reasons that matter to this product: one small binary that runs the
API and the worker in one process on a cheap VPS (`apps/api/src/main.rs:10`), no garbage collector pauses while
many jobs run, and a compiler that refuses many bugs before they ship (a tool name typo, a missing error branch).
The cost is a slower first build and a steeper learning curve. [07. Alternatives](07-alternatives.md) weighs this.

## 2. Ownership and borrowing

**Analogy.** A library book. One person *owns* it (can throw it away). Others can *borrow* it to read (`&T`),
many at once. Only one can borrow it to write notes in the margin (`&mut T`), and nobody else can read while they do.

```rust
pub fn route(f: &MessageFacts, hints: &Hints, agents: &[RoutingAgent]) -> Option<Route>
```
`apps/api/src/agent_registry/routing.rs:176`. The function *borrows* the facts, the hints and a slice of agents.
It does not take them, so the caller still has them after the call. In TypeScript every object is passed by
reference and anybody can mutate it; here the signature tells you nobody will.

```rust
let mut tx = state.db.begin_with("BEGIN IMMEDIATE").await?;
```
`apps/api/src/http/agent_registry_api.rs:504`. `mut` is needed because a transaction changes state when you run
queries on it. Without `mut` the compiler refuses.

**Trade-off.** You will fight the borrow checker for the first week. The reward is that "who can change this?" is
always answered by the type, not by reading the whole file.

**Lifetimes.** When a struct holds a borrow, you see `'a`:
```rust
pub struct SendFacts<'a> { pub to: &'a str, pub participants: &'a [String], ... }
```
`apps/api/src/agents/autonomy.rs:111`. Read it as "this struct must not outlive the strings it points to". It
avoids copying every string for a check that runs on every send. You rarely need to write lifetimes yourself;
you mostly see them in signatures.

**`Clone` and `Arc`.** When sharing is simpler than borrowing, the code clones. `AppState` is `#[derive(Clone)]`
(`apps/api/src/http/mod.rs:59`) and every field is an `Arc<...>` or a pool handle, so cloning is cheap (a
reference count goes up). `Arc<dyn Classifier>` means "a shared pointer to anything that implements Classifier".
This is how the app swaps a real model for a rules fallback without changing callers.

## 3. `Result`, `Option` and `?`

**Analogy.** A function that may fail returns a box with either the value or the error. `?` opens the box: value
out, or return the error to the caller right now.

```rust
pub async fn connect(database_url: &str) -> anyhow::Result<SqlitePool> {
    let options = SqliteConnectOptions::from_str(database_url)
        .with_context(|| format!("invalid DATABASE_URL {database_url}"))?
```
`apps/api/src/db.rs:29`. `with_context` adds a plain sentence to the error, then `?` returns it if it failed.

TypeScript has `try/catch` and exceptions can come from anywhere. Rust puts the failure in the return type, so
the compiler forces you to handle it. `Option<T>` is the same idea for "maybe nothing" (`T | undefined`).

Two error libraries are used, on purpose:
- `anyhow` for "any error, with context", in startup code and jobs.
- `thiserror` for a typed enum when callers must branch on the kind. `AppError`
  (`apps/api/src/error.rs:36`) is the HTTP error type: `Unauthorized`, `Forbidden`, `NotFound`, `SendBlocked`...

```rust
impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match e { sqlx::Error::RowNotFound => AppError::NotFound, other => AppError::Internal(other.into()) }
    }
}
```
`apps/api/src/error.rs:76`. Because of this `From`, a `?` on a SQLx call inside a handler turns "row not found"
into a 404 automatically. `pub type AppResult<T> = Result<T, AppError>` (`error.rs:121`) is the handler return
type everywhere.

## 4. Enums and `match`

**Analogy.** A TypeScript discriminated union, but the compiler checks you handled every case.

```rust
pub enum Source { Manual, Thread, Rule, Label, Personal, Generic, Fallback }

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Manual => "manual",
            ...
```
`apps/api/src/agent_registry/routing.rs:18` and `:35`. Add a variant and forget it in `as_str` and the build
fails. In TypeScript a `switch` on a union compiles fine with a missing case unless you add tricks.

Enums can carry data, which TypeScript unions also do, but Rust pattern-matches on it directly:
```rust
pub enum Step {
    Extract { fields: Vec<ExtractField> },
    DraftReply { quality: QualityTier },
    CreateReview { title: String },
    RemindIfNoReply { hours: u32 },
    ...
```
`apps/api/src/agent_registry/spec.rs:183`. `Step::required_tools` (`spec.rs:199`) matches each variant to the
tools a workflow needs. The `AppError` enum and `JobError` (`apps/api/src/jobs/handlers.rs:27`, `Retry` versus
`Permanent`) are the same pattern: the type *is* the documentation of what can happen.

`if let` and `let ... else` are small matches:
```rust
let Some(owner) = ctx.owner.as_ref().filter(|o| o.routine_ok()) else { return Ok(false) };
```
`apps/api/src/agents/autonomy.rs:230`. Read as "if there is an owner who allows routine replies, continue;
otherwise return false".

## 5. Structs, `impl`, and `derive`

A `struct` is data. An `impl` block adds methods. `#[derive(...)]` asks the compiler to write common code.

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentSpec { pub name: String, pub key: String, ... }
```
`apps/api/src/agent_registry/spec.rs:294`. `Debug` lets you print it, `Clone` copy it, `PartialEq` compare it,
`Serialize`/`Deserialize` turn it to and from JSON. `deny_unknown_fields` is a safety choice: a JSON spec with a
field the code does not know is rejected, so a typo or an injected field never silently does something.

## 6. Traits: interfaces you can swap

**Analogy.** A TypeScript `interface`, plus the ability to attach it to types you did not write.

```rust
#[async_trait]
pub trait Classifier: Send + Sync {
    async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError>;
}
```
`apps/api/src/ai/mod.rs:187`. Three implementations exist: `OpenRouterClassifier`
(`apps/api/src/ai/openrouter.rs:624`), a Jev adapter (`apps/api/src/ai/jev.rs`), and rules fallback
(`apps/api/src/ai/fallback.rs`). `build_state` (`apps/api/src/lib.rs:73`) picks one from config and stores it as
`Arc<dyn Classifier>`. Tests pass their own (`apps/api/tests/common/mod.rs:40`, `BodyDrafter` implements `Drafter`).

This is the single most important pattern in the codebase: **every model call sits behind a trait**, so the
whole app runs offline in CI with scripted doubles (`ScriptedPlanner`, `ScriptedDesigner`, `FixedJudge`).

`Send + Sync` means "safe to share across threads". Tokio runs tasks on many threads, so anything inside
`AppState` must promise this. The compiler checks it.

## 7. `async`/`await` and Tokio

Rust `async fn` returns a future, like a Promise, but it does nothing until something awaits it. Tokio is the
runtime that drives futures (Node has one built in; Rust lets you choose).

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
```
`apps/api/src/main.rs:10`. One multi-thread runtime for the whole process.

```rust
let worker = matches!(mode.as_str(), "serve" | "worker")
    .then(|| tokio::spawn(jobs::worker::run(state.clone(), shutdown_rx.clone())));
```
`apps/api/src/main.rs:24`. `tokio::spawn` is like starting a background task. The worker loop
(`apps/api/src/jobs/worker.rs:14`) uses a `Semaphore` to cap concurrency and `tokio::select!` to wait on "a job
arrived" or "shutdown" at the same time, whichever comes first. `select!` is `Promise.race` with cancellation of
the losers.

**Rule you will see everywhere:** never hold a database transaction across an `await` on a network or model
call. `docs/architecture.md` section 12 says why: SQLite has one writer at a time.

## 8. Axum: handlers, extractors, state

**Analogy.** Express or Fastify, but the handler's parameters *are* the parsing.

```rust
async fn list(State(state): State<AppState>, user: AuthUser, Path(ws): Path<String>) -> AppResult<Json<Value>>
```
`apps/api/src/http/agent_registry_api.rs:198`. Each parameter is an **extractor**:
- `State<AppState>`: the shared app state (DB pool, config, model clients).
- `AuthUser`: the signed-in user. It is a custom extractor (`impl FromRequestParts<AppState> for AuthUser`,
  `apps/api/src/auth.rs:189`) that reads the session token and rejects with 401 if missing. The handler body never
  thinks about auth.
- `Path(ws)`: the `{ws}` segment of the URL.
- `Json(body)`: a parsed and validated JSON body (on `put_spec`, `agent_registry_api.rs:480`).

Routes are registered in `apps/api/src/http/mod.rs:103` (`router`) and in each area's `routes()` function, for
example `apps/api/src/http/designer_api.rs:25`. Middleware is a `layer`: request ids
(`apps/api/src/error.rs:22`, `request_id_middleware`), rate limiting, CORS, security headers.

Return types: `AppResult<Json<Value>>`. On `Err`, `impl IntoResponse for AppError` (`error.rs:85`) turns the
enum into a status code and a `{ error: { code, message, request_id } }` body. The web client reads exactly
this shape (`apps/web/src/api/client.ts`, `parseError`).

## 9. SQLx and SQLite

SQLx runs plain SQL strings with `?1`, `?2` binds. There is no ORM. This matches the product rule that every query
filters on `workspace_id`; you can see it in the SQL.

```rust
sqlx::query_scalar("SELECT COALESCE(SUM(cost_usd), 0.0) FROM ai_calls WHERE workspace_id = ?1 AND created_at >= ?2")
    .bind(workspace_id).bind(today_start_ms()).fetch_one(db).await
```
`apps/api/src/ai_ledger.rs:303`. `query_scalar` for one value, `query_as::<_, Job>` for a struct that derives
`FromRow` (`apps/api/src/jobs/mod.rs:68`), `query` for writes.

**Transactions and `BEGIN IMMEDIATE`.** SQLite in WAL mode allows many readers and one writer. A normal `BEGIN`
starts as a reader and upgrades to a writer at the first write; if another writer got there first, the upgrade
fails at once with `SQLITE_BUSY` instead of waiting. `BEGIN IMMEDIATE` takes the write lock up front, so the
transaction *queues* behind the other writer (up to `busy_timeout`, 15 s, `apps/api/src/db.rs:36`). That is why
the codebase has 143 `begin_with("BEGIN IMMEDIATE")` calls and the first statement inside is always a write.
`apps/api/tests/db_concurrency.rs` hammers the API and several workers on one file and fails if "database is
locked" ever surfaces.

**Pool and pragmas.** `apps/api/src/db.rs:29`: WAL journal, `synchronous = NORMAL`, foreign keys on, 8
connections, owner-only file permissions. Migrations are plain SQL files in `apps/api/migrations/`, applied by
`sqlx::migrate!` at start (`db.rs:72`). SQLx checksums each file, so an applied migration is never edited; you add
the next number.

**Comparison to MongoDB.** You are used to documents and no joins. Here you get one file, real foreign keys with
`ON DELETE CASCADE`, FTS5 full-text search (`knowledge_fts`), and a vector index (`knowledge_vec`, a `vec0`
virtual table from `sqlite-vec`, `apps/api/migrations/0015_knowledge_index.sql`). All in the same transaction as
your business rows. That atomicity is the reason SQLite was chosen over a separate queue and a separate vector DB.

## 10. serde: JSON in and out

`serde` is the JSON library. Attributes shape the mapping:
```rust
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    EmailReceived { #[serde(rename = "match", default)] matcher: TriggerMatch },
    Schedule { every_hours: u32 },
    Manual,
}
```
`apps/api/src/agent_registry/spec.rs:147`. This reads JSON like `{"type":"email_received","match":{...}}`.
`default` fills a missing field with `Default::default()`. `serde_json::json!({...})` builds values inline
(`apps/api/src/ai/openrouter.rs:205`, the OpenRouter request body).

## 11. Modules and the crate layout

`apps/api/src/lib.rs` lists every top-level module (`pub mod agents;` ...). A folder with a `mod.rs` is a module
with children (`agents/mod.rs` lists `approvals`, `autonomy`, `case`...). `pub` makes an item visible outside its
module; `pub(crate)` inside the crate only; `pub(super)` to the parent (`agent_registry_api.rs`, `detail_json`).

`main.rs` is a thin binary that calls into the library. A second binary, `apps/api/src/bin/selftest.rs`, reuses
the same library for the daily model probe. Integration tests in `apps/api/tests/*.rs` also link the library,
which is why `build_state` and `http::router` are public.

## 12. Tests

Unit tests live next to the code in a `#[cfg(test)] mod tests` block (`apps/api/src/agent_registry/routing.rs:374`).
They can call private functions. Integration tests live in `apps/api/tests/`, boot a full app with an in-memory
SQLite and scripted AI doubles (`apps/api/tests/common/mod.rs`), and drive it through `tower::ServiceExt::oneshot`
without opening a port.

```rust
#[tokio::test]
async fn golden_scenarios_hold_the_safety_invariants() { ... }
```
`apps/api/tests/golden_offline.rs:37`. `#[tokio::test]` gives the test an async runtime. `#[test]` is for sync
code. `#[ignore]` marks tests that cost money (`golden_live.rs`); run them with `-- --ignored`.

`cargo clippy --all-targets -- -D warnings` is part of CI (`.github/workflows/deploy.yml:37`). Clippy is ESLint
with stronger opinions; the build fails on any warning.

## In this repo

| Idea | Best file to read |
|---|---|
| Startup, runtime, graceful shutdown | `apps/api/src/main.rs` |
| Error type and HTTP mapping | `apps/api/src/error.rs` |
| Pool, pragmas, migrations, ids | `apps/api/src/db.rs` |
| A pure function with enums and tests | `apps/api/src/agent_registry/routing.rs` |
| A data contract with serde | `apps/api/src/agent_registry/spec.rs` |
| Traits with real and test implementations | `apps/api/src/ai/mod.rs`, `apps/api/src/designer/mod.rs` |
| A worker loop with Tokio | `apps/api/src/jobs/worker.rs` |
| An HTTP area with extractors and transactions | `apps/api/src/http/agent_registry_api.rs` |
| A full-app integration test | `apps/api/tests/common/mod.rs`, `apps/api/tests/routing.rs` |

## Exercises

1. **Read a function aloud.** Open `apps/api/src/agents/autonomy.rs:45`, `effective_level`. Translate the `fold`
   into a `for` loop in your head, then into TypeScript `reduce`. Explain why the return type is a struct and not
   just a number.
2. **Make the compiler talk.** In `apps/api/src/agent_registry/routing.rs`, add a new variant `Source::Test` and
   run `cargo build --manifest-path apps/api/Cargo.toml`. Read every error. Fix them. Then remove the variant.
   Lesson: the compiler lists every `match` you must update.
3. **A route with a test.** Add `GET /workspaces/{ws}/agents/{id}/scopes` that returns the active spec's
   `scopes` as JSON. Steps: handler in `apps/api/src/http/agent_registry_api.rs` using `visible_agent` and
   `versions::active_spec_in`; register in `routes()`; write an integration test in a new
   `apps/api/tests/scopes.rs` using `tests/common/mod.rs` (copy how `tests/routing.rs` creates a workspace and
   agent). Run `cargo test --test scopes` and `cargo test --test tenant_isolation` (your route must check
   membership or that test fails). Run clippy. Then revert if you do not want to keep it.
4. **Transactions.** Find one `begin_with("BEGIN IMMEDIATE")` where the first statement is an `UPDATE` and one
   where it is an `INSERT`. Now find `send_draft_held` (`apps/api/src/replies.rs:509`) and note where the model
   call (`send_safety::check`) happens relative to the transaction. Why there?

## Check yourself

1. In a handler signature, what does `State(state): State<AppState>` do, and where does `AppState` come from?
2. Why does the codebase use `BEGIN IMMEDIATE` instead of plain `BEGIN`?
3. How does a `sqlx::Error::RowNotFound` become a 404 without any `if` in the handler?

<details>
<summary>Show answers</summary>

1. It is an Axum extractor that pulls the shared application state out of the router. `AppState` is built once in `build_state` (`apps/api/src/lib.rs:73`) from `Config`, holds the pool and every model client behind `Arc<dyn Trait>`, and is passed to `http::router(state)` (`apps/api/src/http/mod.rs:103`). Cloning it is cheap because every field is a shared pointer.
2. SQLite has one writer at a time. A plain `BEGIN` starts as a read transaction and tries to upgrade on the first write; if another writer holds the lock the upgrade fails immediately with `SQLITE_BUSY`. `BEGIN IMMEDIATE` takes the write lock at the start, so the transaction waits in line (up to the 15 s `busy_timeout` in `apps/api/src/db.rs:36`) instead of failing. `apps/api/tests/db_concurrency.rs` proves no "database is locked" error leaks.
3. `impl From<sqlx::Error> for AppError` (`apps/api/src/error.rs:76`) maps `RowNotFound` to `AppError::NotFound`. The `?` operator calls `From` automatically when the error types differ. Then `impl IntoResponse for AppError` (`error.rs:85`) maps `NotFound` to status 404 and the JSON error body.
</details>

**Next:** [02. Architecture](02-architecture.md).
