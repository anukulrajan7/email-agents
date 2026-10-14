# L3 — Best Rust principles, checked against our code

**Summary.** Fourteen principles experienced Rust engineers apply. For each: what it means, where our code already follows it, where it breaks it, and the fix. Use this page as a review checklist for every Rust change. Paths are under `apps/api/src/`.

| # | Principle | We follow it | We break it | Fix |
|---|---|---|---|---|
| 1 | **Make invalid states unrepresentable.** Use enums and types so wrong combinations cannot compile. | `Step`, `Trigger`, `Role`, `AppError`, `AiError` are enums; `AgentSpec` uses `deny_unknown_fields` | Statuses compared as strings: `case.status == "waiting_approval"` (`agents/runner.rs:148`), `a.row.status == "active"` (`routing.rs:177`), job `status` text | `enum CaseStatus`, `enum AgentStatus` with `sqlx::Type`; the database `CHECK` constraint stays |
| 2 | **Parse, don't validate.** Turn raw input into a typed value once at the edge; inside, trust the type. | `Config::from_env` parses every env var into typed fields and fails startup on bad numbers; agent specs parse into `AgentSpec` | Job payloads stay `serde_json::Value` and are read with `str_field(&payload, "message_id")` in three copies (`jobs/handlers.rs:109` and others) | One `#[derive(Deserialize)] struct` per job kind, parsed once in `handlers::handle` |
| 3 | **Errors are values; panics are bugs.** Return `Result`; reserve `panic`/`unwrap` for "impossible" states. | `AppError`, `JobError`, `?` everywhere; regex `expect("valid regex")` in `LazyLock` is fine (checked at first use, constant input) | `expect("object")` in `agents/runner.rs:268`; `Mutex::lock().expect(...)` on every login (`sessions.rs`); panics inside jobs are not recorded (`worker.rs:39`) | Return errors; capture panics from the `JoinHandle` |
| 4 | **Let the type system enforce security.** If a rule must always hold, make the compiler check it. | `AuthUser` extractor: a handler cannot read the user without authentication | `require_role` is a manual call in 159 places | `Member` extractor that proves membership before the handler runs |
| 5 | **Newtypes give meaning to primitives.** Wrap a `String` in a named type so different ids cannot be mixed. | Prefixed ids (`msg_…`, `job_…`) help humans | Every id is `String`; a `message_id` can be passed where a `workspace_id` is expected; secrets are plain `String` in a `Debug` struct (`config.rs:37`) | `struct WorkspaceId(String)`, `struct Secret(String)` with a redacting `Debug` |
| 6 | **RAII: tie cleanup to scope.** Resources free themselves when their owner is dropped. | Transactions roll back on drop; semaphore permits release on drop; the pool closes at shutdown | Dropping the job `JoinHandle` also drops the panic information | Keep the handle and await it |
| 7 | **Borrow by default, own when you must, clone deliberately.** | `route(&MessageFacts, &Hints, &[RoutingAgent])` borrows everything; `AppState` clones are `Arc` bumps | Some long functions clone strings repeatedly to satisfy the borrow checker (look for `.clone()` inside loops in `jobs/handlers.rs`) | Restructure so one owner lives long enough; pass `&str` |
| 8 | **Never block the async runtime.** | Argon2 and document parsing use `spawn_blocking`; network calls are async | `std::fs` in `backup.rs:36-70`; CSV parsing inside an async fn in `team/people.rs::import_csv` | `tokio::fs` or `spawn_blocking` |
| 9 | **Never hold a lock or transaction across an `.await` on the network.** | Stated rule; model calls happen outside transactions (`handlers.rs:265`) | None found | Keep it; add to the PR checklist |
| 10 | **Keep `unsafe` tiny, isolated and justified.** | One block, in `db.rs:15-26`, behind a safe function and `Once` | No `// SAFETY:` comment | Add the comment explaining why the transmute is sound |
| 11 | **Small functions, cohesive modules.** A function does one thing; a module owns one concept. | Most modules are cohesive (`jobs/`, `agents/`, `designer/`) | Functions of 120 to 270 lines: `build_state` (`lib.rs:73`), `persist_message` (`ingest.rs:32`), `commit` (`agents/runner.rs:514`), `import_csv`; `jobs/handlers.rs` is 1,514 lines | Extract named steps; one file per job family |
| 12 | **Don't repeat yourself for invariants.** Encode a rule once. | `jobs::enqueue` is the single way to add a job | `begin_with("BEGIN IMMEDIATE")` repeated 143 times; `finalize_sent`, `job_err`, `str_field`, `workspace_of` duplicated | `db::write_tx(&pool)` helper; shared send module |
| 13 | **Lint and format as gates.** | `cargo clippy --all-targets -- -D warnings` in CI and in `make check` | No `cargo fmt --check` in CI (HYPOTHESIS: not found in the workflows) | Add `cargo fmt --check` to CI |
| 14 | **Test behaviour through public seams, and keep tests fast.** | Integration tests call the real router with `tower::ServiceExt::oneshot` and drain real jobs; external APIs are local mock servers | 60 separate test binaries; every test builds a fresh database and runs 44 migrations; suite is slow | One integration-test binary; a pre-migrated template database copied per test; `cargo nextest` |

## How to use this in a review

Read a Rust diff three times:

1. **Correctness:** errors returned, not unwrapped? Transactions start with `BEGIN IMMEDIATE`, contain no network calls, and commit? Every `/workspaces/{ws}` handler checks membership?
2. **Types:** could a string be an enum? Could two ids be swapped? Is raw JSON parsed into a struct at the edge?
3. **Runtime:** anything blocking inside async? Any lock held across `.await`? Any spawned task whose result is ignored?

## Check questions

- Pick principle 1. Write the enum you would add for case status, and list every file that compares `status` strings today.
- Pick principle 5. Which bug class disappears if `WorkspaceId` and `MessageId` are different types?
- Pick principle 14. Estimate how much link time disappears if 60 test binaries become one.

## References

[Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) · [Clippy lints](https://rust-lang.github.io/rust-clippy/master/) · "Parse, don't validate" by Alexis King · [Rust Book ch. 9 (errors)](https://doc.rust-lang.org/book/ch09-00-error-handling.html) · [Tokio: bridging with sync code](https://tokio.rs/tokio/topics/bridging)
