# Learn — start here

**Who this is for.** You write React, Next, TypeScript and Node every day. You have not written Rust, and you have not built an AI agent from the inside. Post Room was built in two days by AI agents, so the fastest way to own it is to learn Rust and agent design *through this exact code*. Every page below uses our files as the examples, compares Rust ideas to the TypeScript you already know, and ends with questions you should be able to answer out loud.

## The path

| Step | Page | You can do this after | Time |
|---|---|---|---|
| 1 | [L1 Rust basics](10-rust-basics.md) | Read any file in `apps/api/src` and say what it does | 4 to 6 h |
| 2 | [L2 Rust intermediate and advanced](11-rust-advanced.md) | Explain the router, the job worker and the database layer; write a handler with a test | 8 to 10 h |
| 3 | [L3 Rust principles](12-rust-principles.md) | Review a Rust pull request and name what is wrong and why | 2 h |
| 4 | [L4 AI agent development](13-ai-agent-development.md) | Explain how one email becomes a safe sent reply, and what each model call costs | 6 to 8 h |
| 5 | [L5 Glossary](14-glossary.md) | Look up any word in the code or in these docs | reference |
| 6 | [08 Personal learning roadmap](08-personal-learning-roadmap.md) | Ship real fixes while you learn: 18 lessons, each tied to a defect | 6 weeks |

Go in order for 1 and 2. Read 3 after 2. Read 4 any time after 1. Keep 5 open in a side tab.

## How to study one page

1. Read one section, not the whole page.
2. Open the file it names in your editor and find the exact line. Lines move; search for the function name.
3. Answer the "Check" question in your own words, out loud or written. If you cannot, read the section again.
4. Run something: `cargo test --test <name>` for the module, or add a `println!` and watch it.
5. Write one line in `~/learning/mistakes.md`: what surprised you.

## Commands you will use every day

```bash
cd apps/api
cargo check                      # fast: type-check only, no binary
cargo clippy --all-targets       # lints (CI fails on any warning)
cargo fmt                        # format
cargo test --lib                 # unit tests inside src/ (fast)
cargo test --test auth           # one integration test file (tests/auth.rs)
cargo test --test agents -- case_waits_for_approval   # one test by name
cargo doc --open                 # browsable docs for our crate and every dependency
make api                         # run API + worker locally (from repo root)
```

## Deeper material already in the repo

`docs/learn/` has a nine-module curriculum (vision, Rust, architecture, tools, AI basics, agentic architecture, evals, alternatives, resources). These Learn pages are shorter and more direct; use `docs/learn/` when you want more examples.

## Official references

[The Rust Book](https://doc.rust-lang.org/book/) · [Rust by Example](https://doc.rust-lang.org/rust-by-example/) · [Rustlings exercises](https://github.com/rust-lang/rustlings) · [Tokio tutorial](https://tokio.rs/tokio/tutorial) · [axum docs](https://docs.rs/axum/latest/axum/) · [sqlx docs](https://docs.rs/sqlx/latest/sqlx/) · [Anthropic: Building effective agents](https://www.anthropic.com/engineering/building-effective-agents) · [OpenRouter docs](https://openrouter.ai/docs)
