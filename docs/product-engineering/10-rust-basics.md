# L1 — Rust basics, through our code

**Summary.** Twelve ideas cover almost every line in `apps/api/src`. Each section gives the idea in plain words, the TypeScript you already know, the place in our code, and a check question. Paths are under `apps/api/src/`.

---

## 1. Cargo, crates and modules

- **Cargo** is npm plus tsc plus jest in one tool. `Cargo.toml` is `package.json`; `Cargo.lock` is the lockfile.
- A **crate** is one compiled package. Ours is `email-ai-api`: a library (`src/lib.rs`) plus a binary (`src/main.rs`) plus a second binary (`src/bin/selftest.rs`).
- A **module** is a namespace. `lib.rs` lists them: `pub mod jobs;` means "compile `jobs/mod.rs` and make it public". Inside the crate you import with `use crate::jobs::kind;` (like `import { kind } from '@/jobs'`).
- Visibility is private by default. `pub` = exported; `pub(crate)` = visible inside our crate only (see `pub(crate) async fn chat_json` in `ai/openrouter.rs:154`).

**Check:** why can `tests/auth.rs` call `email_ai_api::http::router` but not a private helper inside `http/messages.rs`?

## 2. Variables, mutability and basic types

- `let x = 5;` is immutable, like `const`. `let mut x = 5;` can change.
- Numbers have sizes: `i64` (we store timestamps as unix milliseconds in `i64`, see `db::now_ms` in `db.rs:89`), `u32` (`max_tokens`), `f64` (dollar costs), `usize` (lengths and indexes).
- `bool`, `char`, and tuples like `(String, String)` (used for two-column query results, `auth.rs:67`).

**Check:** why is cost stored as `f64` but time as `i64`?

## 3. Strings: `String` versus `&str`

- `String` is an owned, growable string on the heap. `&str` is a borrowed view into some string.
- Function parameters usually take `&str` so callers can pass either: `pub fn new_id(prefix: &str) -> String` (`db.rs:110`) borrows the prefix and returns a new owned `String`.
- `format!("{prefix}_{}", ...)` builds a `String` like a template literal.
- `.to_string()` and `String::from` turn a `&str` into a `String` (an allocation).

**Check:** `new_id` returns `String`, not `&str`. What would go wrong if it tried to return a `&str` to a value it created inside the function?

## 4. Structs and `impl`

A struct is a typed object; methods live in an `impl` block.

```rust
pub struct Job { pub id: String, pub workspace_id: Option<String>, pub kind: String, ... }
impl Job {
    pub fn payload(&self) -> Value { serde_json::from_str(&self.payload_json).unwrap_or(Value::Null) }
}
```
(`jobs/mod.rs:67-85`). `&self` is like `this`, borrowed read-only. A method taking `&mut self` may change the struct; `self` (no `&`) consumes it.

**Check:** why does `payload` take `&self` and not `self`?

## 5. Enums and `match`

Rust enums are TypeScript discriminated unions with the compiler checking every case.

```rust
pub enum Step {
    Extract { fields: Vec<ExtractField> },
    DraftReply { quality: QualityTier },
    CreateReview { title: String },
    RemindIfNoReply { hours: u32 },
    SaveToGmailDrafts,
    ...
}
```
(`agent_registry/spec.rs:183`). A `match` must handle every variant or the code does not compile. See `AppError::into_response` (`error.rs:85-103`): every error variant maps to exactly one HTTP status. Add a variant and forget the status, and `cargo check` fails.

**Check:** in TypeScript, what stops you forgetting a case in a `switch` on a union? What stops you in Rust?

## 6. `Option<T>` — there is no `null`

`Option<T>` is either `Some(value)` or `None`. You must handle `None` before you can use the value.

- `workspace_id: Option<String>` on `Job`: system jobs have no workspace.
- `fetch_optional` returns `Option<Row>` (`auth.rs:70`).
- Useful tools: `if let Some(x) = opt { ... }`, `opt.unwrap_or(default)`, `opt.map(|x| ...)`, `opt.as_deref()` (turns `Option<String>` into `Option<&str>`), and `let Some(x) = opt else { return ... };` (let-else, used in `routing.rs:342`).

**Check:** what does `let Some((facts, thread_id)) = message_facts(...).await? else { return Ok(None) };` do in plain words?

## 7. `Result<T, E>` and `?` — errors are values

A function that can fail returns `Result<T, E>`: `Ok(value)` or `Err(error)`. There are no exceptions.

- `?` means "if this is `Err`, return it from my function now; else unwrap the `Ok`". It is like `await` for errors.
- Handlers return `AppResult<T>`, which is `Result<T, AppError>` (`error.rs:121`).
- `?` converts error types automatically through `From`: a `sqlx::Error` becomes an `AppError` because of `impl From<sqlx::Error> for AppError` (`error.rs:76-83`).
- `.unwrap()` and `.expect("msg")` crash the program on `Err`/`None`. Fine in tests; avoid in request paths.

**Check:** a query in a handler fails with "row not found". Trace what HTTP status the client gets, file by file.

## 8. Ownership and borrowing (the big one)

Every value has exactly one owner. When the owner goes out of scope, the value is freed. No garbage collector.

- **Move:** `let b = a;` moves a `String` from `a` to `b`; `a` can no longer be used.
- **Borrow:** `&a` lends read access; `&mut a` lends write access. Many readers or one writer, never both.
- Our routing function only borrows: `pub fn route(f: &MessageFacts, hints: &Hints, agents: &[RoutingAgent]) -> Option<Route>` (`agent_registry/routing.rs:176`). The caller keeps its data.
- `.clone()` makes a full copy when you truly need a second owner. Cloning `AppState` is cheap because its fields are `Arc` pointers (see L2).

Analogy: a library book. One owner. Many people may read it at once, or one person may write in the margins, not both.

**Check:** in `jobs/worker.rs:37`, the code does `let state = state.clone();` before `tokio::spawn(async move { ... })`. Remove the clone in your head: what does the compiler complain about, and why?

## 9. Collections, closures and iterators

- `Vec<T>` is an array; `HashMap<K, V>` is a `Map`; slices `&[T]` are borrowed views of a `Vec`.
- Closures are arrow functions: `|a| a.row.status == "active"`.
- Iterators chain like `.filter().map()` in JS, but are lazy and compile to a plain loop (no extra arrays until `.collect()`):

```rust
let mut matches: Vec<(&RoutingAgent, Class, usize)> = agents
    .iter()
    .filter(usable)
    .filter(|a| a.row.kind != "lead" && !a.row.is_personal())
    .filter_map(|a| best_match(a, f).map(|(c, n)| (a, c, n)))
    .collect();
```
(`agent_registry/routing.rs:182-187`). `filter_map` = filter and map in one step, keeping only `Some` results.

**Check:** why does this chain produce `Vec<(&RoutingAgent, ...)>` with a reference, and not owned copies of the agents?

## 10. `derive` and serde

`#[derive(...)]` asks the compiler to write common code for you.

- `#[derive(Debug, Clone, FromRow)] pub struct Job` (`jobs/mod.rs:66`): printable with `{:?}`, cloneable, and sqlx can fill it from a database row.
- `#[derive(Serialize, Deserialize)]` (serde) converts to and from JSON. `#[serde(deny_unknown_fields)]` on agent specs rejects any unexpected key, so a typo or an injected field fails loudly.
- `#[derive(thiserror::Error)]` writes error messages for `AppError` from `#[error("...")]` attributes.

**Check:** what would happen if `AgentSpec` did *not* have `deny_unknown_fields` and a model returned `"send_to_all": true`?

## 11. Traits, briefly (L2 goes deeper)

A trait is an interface. `pub trait Classifier: Send + Sync { async fn classify(&self, input: &ClassifyInput) -> Result<ClassifyOutput, AiError>; }` (`ai/mod.rs:187`). Three types implement it: the OpenRouter classifier, the Jev classifier, and a rules fallback. The rest of the code only knows "something that can classify".

**Check:** where in `lib.rs` is the decision made about which classifier we actually use?

## 12. Tests

- Unit tests live next to code in `#[cfg(test)] mod tests { ... }` (see `jobs/mod.rs:290`). `#[cfg(test)]` means "compile only when testing".
- `#[test]` for plain functions; `#[tokio::test]` for async ones.
- Integration tests live in `tests/*.rs`. Each file is compiled as its own program that uses our crate as a library. We have 60 such files; that is part of why the suite is slow to build (see the test-speed plan).
- Run one: `cargo test --test auth`. Run one by name: `cargo test --test auth -- login_rate_limit`.

**Check:** why can a unit test inside `jobs/mod.rs` call a private function, but `tests/pipeline.rs` cannot?

---

## Exercises (in order)

1. Open `db.rs` and explain every line of `new_id` and `now_ms`.
2. In `error.rs`, add a variant `Gone` that maps to HTTP 410 with code `"gone"`. Run `cargo check`. Then remove it.
3. In `routing.rs`, rewrite the `matches` chain as a `for` loop on paper. Which version is easier to read for you, and why?
4. Run `cargo test --test routing` and read one test top to bottom.

## Read next

L2 for lifetimes, traits in depth, async, axum and sqlx. Rust Book chapters 3 to 10 cover this page in more detail.
