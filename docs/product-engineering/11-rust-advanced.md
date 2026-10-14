# L2 — Rust intermediate to advanced, through our code

**Summary.** This page covers what makes our backend work: lifetimes, traits and trait objects, error design, shared state across threads, async with tokio, the axum web framework, sqlx with SQLite, macros, and the one `unsafe` block. Read L1 first. Paths are under `apps/api/src/`.

---

## 1. Lifetimes: how long a borrow is valid

A lifetime is a name for "how long this reference is allowed to live". Usually the compiler infers it. You see it written only when a struct holds a reference.

```rust
pub struct SendFacts<'a> { pub to: &'a str, pub participants: &'a [String], ... }
```
(`agents/autonomy.rs`). Read `'a` as: "a `SendFacts` cannot outlive the strings it points to". This lets the autonomy check look at the draft's recipient and the thread's participants **without copying them**. `ToolCtx<'_>` in `agents/tools.rs` is the same idea with an anonymous lifetime.

Why it matters: Rust frees memory when the owner goes away. Lifetimes are how the compiler proves no reference ever points at freed memory. In TypeScript the garbage collector keeps it alive for you instead.

**Check:** why do most of our structs hold `String` (owned) and not `&str` (borrowed)?

## 2. Traits, generics and trait objects

- **Trait** = interface. **Generic** = a type parameter, like `T` in TypeScript.
- Two ways to use a trait:
  - **Generic (static dispatch):** `fn get_in(db: impl SqliteExecutor<'_>, ...)` in `agent_registry/store.rs`. The compiler makes a specialised copy per concrete type. Fastest, but more code to compile.
  - **Trait object (dynamic dispatch):** `Arc<dyn Classifier>` in `AppState` (`http/mod.rs:66`). One copy of the calling code; the actual method is looked up at runtime through a pointer table (a "vtable").
- We use trait objects for every AI port (classifier, drafter, planner, critic, judge, embedder...). `build_state` (`lib.rs:73-342`) picks the implementation from config: OpenRouter if a key exists, otherwise rules or no-op fallbacks. Tests pass scripted fakes, e.g. `ScriptedPlanner` in `evals/safety.rs`.
- `#[async_trait]` (crate `async-trait`) is needed because an `async fn` in a trait used as `dyn` must return a boxed future.

**Check:** why is `dyn` the right choice for the classifier, but `impl SqliteExecutor` the right choice for a store function?

## 3. Error design: `thiserror` versus `anyhow`

- **`thiserror`** for errors callers must match on: `AppError` (`error.rs:37`) has one variant per HTTP outcome; `AiError::{Retryable, Permanent}` (`ai/mod.rs:179`) tells the job queue whether to retry; `JobError::{Retry, Permanent}` (`jobs/handlers.rs`).
- **`anyhow`** for "something went wrong, add context, bubble up": `main` returns `anyhow::Result<()>` and uses `.context("binding {addr}")`.
- Conversions with `From` decide how low-level errors become high-level ones. The design rule: **the type of the error carries the decision** (retry or not, 404 or 500), not a string.

**Check:** why would `AiError(String, bool)` be a worse design than two enum variants?

## 4. Sharing state across threads: `Arc`, `Mutex`, `RwLock`, `OnceLock`, `LazyLock`

- **`Arc<T>`** = Atomically Reference Counted pointer. Many owners share one read-only value; cloning bumps a counter. `AppState` is 27 `Arc` fields, so cloning it into every request and job costs almost nothing.
- **`Mutex<T>`** = one writer at a time. Used for the in-memory login limiter (`sessions.rs`). Note: a `std::sync::Mutex` must never be held across an `.await`.
- **`RwLock<T>`** = many readers or one writer. The OIDC key cache uses `tokio::sync::RwLock` (`auth.rs:39`): most requests read, a refresh writes.
- **`LazyLock`** = compute once, on first use. Our regexes: `static SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9_]{1,63}$").expect("valid regex"));` (`agent_registry/spec.rs:17`). **`OnceLock`** is the same but you set it yourself (`sessions.rs` dummy password hash).

**Check:** the login limiter uses `Mutex::lock().expect(...)`. What is a "poisoned" mutex, and why does that make `expect` risky there?

## 5. `Send` and `Sync`: thread safety in the type system

- `Send`: the value can move to another thread. `Sync`: a reference to it can be shared between threads.
- Tokio may run a task on any thread, so everything a spawned task holds must be `Send`. That is why our traits say `pub trait Classifier: Send + Sync`.
- `Rc` (non-atomic counter) is not `Send`; `Arc` is. If you accidentally keep an `Rc` or a `MutexGuard` alive across `.await` in a spawned task, the compiler refuses.

**Check:** why does the compiler, not a code reviewer, catch "this value is not safe to share between threads"?

## 6. async/await and futures

- An `async fn` returns a **future**: a value that describes work, not the work itself. **Futures are lazy**: nothing runs until something `.await`s or spawns it. (JS Promises start immediately; Rust futures do not.)
- A **runtime** drives futures. Ours is tokio, started by `#[tokio::main]` on `main` (`main.rs:9`).
- A task yields only at `.await`. A long synchronous loop or a blocking file read inside async code stalls every other task on that thread. That is why Argon2 hashing runs in `tokio::task::spawn_blocking` (`sessions.rs:49`) and why `backup.rs` using `std::fs` is flagged in the audit.

**Check:** you call `classify(...)` but forget `.await`. What happens at compile time, and what would happen in JavaScript?

## 7. Tokio building blocks we use

| Tool | What it does | Where |
|---|---|---|
| `tokio::spawn` | Start a task that runs concurrently; returns a `JoinHandle` | `jobs/worker.rs:39` (one task per job) |
| `tokio::select!` | Wait on several futures, take the first to finish | `worker.rs:30-33` (permit vs shutdown), `main.rs:63` |
| `Semaphore` | Limit concurrency to N | `worker.rs:17` (job concurrency), `sessions.rs` (4 Argon2 at once) |
| `Notify` | Wake a waiting task | `AppState.jobs_notify`: a handler enqueues a job and wakes the worker |
| `watch` channel | Broadcast a changing value | `main.rs:25` shutdown flag |
| `JoinSet` | Run many tasks, collect results | `sources/nango.rs:899` (8 Gmail fetches at once) |
| `spawn_blocking` | Run blocking/CPU work off the async threads | Argon2, PDF and spreadsheet parsing |
| `task_local!` | A value visible to everything inside one task | `ai_ledger.rs:86` (which workspace to bill), `error.rs:12` (request id) |

**Check:** the worker drops the `JoinHandle` returned by `tokio::spawn`. What information is lost if the job panics? (This is audit defect C2.)

## 8. axum: the web framework

- **Router** maps method + path to a handler: `.route("/workspaces/{ws}/drafts/{id}/send", post(crate::replies::send))` (`http/mod.rs:148`).
- **Handler** = an async function whose parameters are **extractors**: `State(state): State<AppState>`, `user: AuthUser`, `Path((ws, id)): Path<(String, String)>`, `Json(body)` or `Bytes`. axum runs each extractor before your function; if one fails, its error becomes the response. The body extractor must be last because it consumes the request.
- **Custom extractor:** `impl FromRequestParts<AppState> for AuthUser` (`auth.rs:189-196`) reads the token header and loads the user. Any handler that asks for `AuthUser` is authenticated automatically.
- **Responses:** anything implementing `IntoResponse`. `AppError` implements it (`error.rs:85`), so `return Err(AppError::NotFound)` becomes a 404 JSON body.
- **Middleware (layers):** `middleware::from_fn(request_id_middleware)` wraps every request. The last `.layer()` you add runs first on the way in.
- **State:** `.with_state(state)` at the end gives every handler access to `AppState`.
- **merge vs nest:** `merge` combines routers at the same prefix (our 13 feature modules); `nest("/api/v1", api)` mounts under a prefix.

**Check:** authentication is enforced by the type system (`AuthUser`), but workspace membership is a function call (`require_role`) in 159 places. How would a `Member` extractor change that?

## 9. sqlx and SQLite

- **Pool:** `SqlitePool` with 8 connections, WAL mode, 15 s busy timeout (`db.rs:28-50`).
- **Queries:** `sqlx::query("UPDATE ...").bind(x).execute(&db)`, `query_as::<_, Job>(...)` into a `FromRow` struct, `query_scalar` for one value, `fetch_one`, `fetch_optional`, `fetch_all`.
- **Transactions:** `let mut tx = db.begin_with("BEGIN IMMEDIATE").await?;` then `.execute(&mut *tx)`, then `tx.commit().await?`. Dropping `tx` without commit rolls back automatically (RAII).
- **Why `BEGIN IMMEDIATE`:** SQLite allows one writer. A normal transaction that reads first and writes later can fail with `SQLITE_BUSY` when it tries to upgrade its lock. `IMMEDIATE` takes the write lock at the start, so writers wait in line instead of failing.
- **`RETURNING`:** the job claim is one atomic `UPDATE ... RETURNING ...` (`jobs/mod.rs:186-207`), so two workers can never claim the same job.
- **Migrations:** `sqlx::migrate!("./migrations")` embeds the SQL files in the binary at compile time (`db.rs:72`).

**Check:** why does `jobs::enqueue` take `&mut SqliteConnection` instead of the pool?

## 10. Macros

- **Declarative macros** (`macro_rules!`) are pattern-based code templates: `format!`, `vec!`, `matches!`, `tokio::select!`, `tokio::task_local!`.
- **Procedural macros** run Rust code at compile time: `#[derive(...)]`, `#[tokio::main]`, `#[async_trait]`, `sqlx::migrate!`.
- You read macros far more often than you write them. When an error points inside one, look at the input you gave it.

**Check:** `sqlx::migrate!` reads files at compile time. What happens if someone edits an already-applied migration file and deploys?

## 11. `unsafe` and FFI

`unsafe` lets you do things the compiler cannot verify, such as calling C functions. Our only `unsafe` block registers the sqlite-vec C extension with SQLite (`db.rs:15-26`), using `std::mem::transmute` to cast a function pointer. Rules we follow: keep it tiny, wrap it in a safe function, run it once (`std::sync::Once`). Missing: a `// SAFETY:` comment explaining why it is sound.

**Check:** why is `unsafe` acceptable here and nowhere else in the crate?

## 12. Newer syntax you will meet (edition 2024)

- **let-else:** `let Some(x) = opt else { return Ok(None) };` (`routing.rs:342`).
- **if-let chains:** `if let Some(parent) = path.parent() && !parent.as_os_str().is_empty() { ... }` (`db.rs:38`).
- **`matches!`:** `matches!(mode.as_str(), "serve" | "worker")` (`main.rs:26`) returns a `bool`.

## 13. Performance and compile time

- Iterators, generics and `Option` are "zero-cost": they compile to the same machine code as a hand-written loop.
- Avoid needless `.clone()` of big values in hot paths; borrow instead.
- **Compile time is the real cost of Rust.** Generics are copied per type; every integration test file is linked as its own program. With 60 test files, the linker builds 60 programs that each contain the whole crate. That is the main reason the test suite is slow, and the first fix in the test-speed plan.

---

## Exercises

1. Draw `AppState` on paper: which fields are `Arc<dyn ...>`, which are `Option<Arc<...>>`, and why some are optional.
2. In `jobs/worker.rs`, write in comments what each `select!` branch does.
3. Write the `Member` extractor described in section 8 (it is fix P2.3 in the roadmap).
4. Run `EXPLAIN QUERY PLAN` on the claim query against a copy of the dev database.

## Read next

L3 for principles, L4 for the AI side. Rust Book chapters 10, 13, 15, 16 and 17; the Tokio tutorial; axum's `examples/` folder.
