# 08. Resources

Curated per module. Only resources known to exist; official URLs where there is one. Level: **B** beginner,
**I** intermediate, **A** advanced. Pick one book and one course at a time; do not collect.

**Reading time.** 15 minutes to choose. Months to finish.

## Module 01: Rust

| Resource | Type | Level | Why |
|---|---|---|---|
| *The Rust Programming Language* ("the Book"), Klabnik and Nichols. https://doc.rust-lang.org/book/ | Free book | B | The standard start. Chapters 4 (ownership), 6 (enums), 9 (errors), 10 (traits), 16 (concurrency) map to module 01 directly. |
| *Rust by Example*. https://doc.rust-lang.org/rust-by-example/ | Free, runnable | B | Read a page, run it, change it. Good for `match`, `Option`, `Result`, `?`. |
| Rustlings. https://github.com/rust-lang/rustlings | Exercises | B | Small compiler-driven drills. Do them alongside the Book. |
| *Zero To Production in Rust*, Luca Palmieri. https://www.zero2prod.com/ | Book (paid) | I | Builds a real web backend (Actix, SQLx, Postgres, Docker, CI). The closest thing to this repo in book form. |
| Tokio tutorial. https://tokio.rs/tokio/tutorial | Official docs | I | `spawn`, `select!`, channels, graceful shutdown. Read before touching `jobs/worker.rs`. |
| Axum docs and examples. https://docs.rs/axum/latest/axum/ and https://github.com/tokio-rs/axum/tree/main/examples | Official | I | Extractors, state, middleware, errors. The `examples/` folder is the best part. |
| SQLx docs. https://docs.rs/sqlx/latest/sqlx/ | Official | I | `query`, `query_as`, transactions, `migrate!`. |
| *Rust for Rustaceans*, Jon Gjengset | Book (paid) | A | Lifetimes, traits, async internals, once the Book is done. |
| Jon Gjengset's "Crust of Rust" videos. https://www.youtube.com/@jonhoo | Video | A | Long, deep, honest live coding. |
| Clippy lints list. https://rust-lang.github.io/rust-clippy/master/ | Reference | I | When CI fails on a lint you have never seen. |

## Module 02: Architecture, SQLite, systems

| Resource | Type | Level | Why |
|---|---|---|---|
| *Designing Data-Intensive Applications*, Martin Kleppmann | Book (paid) | I/A | Chapters 7 (transactions), 8 (distributed trouble), 11 (streams). Explains why "at-least-once plus idempotency" is the right contract for the jobs table. |
| SQLite: Write-Ahead Logging. https://www.sqlite.org/wal.html | Official | I | Why readers never block and why there is one writer. |
| SQLite: Transactions and `BEGIN IMMEDIATE`. https://www.sqlite.org/lang_transaction.html | Official | I | The exact semantics module 01 section 9 relies on. |
| SQLite: `busy_timeout`, `VACUUM INTO`. https://www.sqlite.org/pragma.html#pragma_busy_timeout and https://www.sqlite.org/lang_vacuum.html | Official | I | Pool settings and backups. |
| SQLite FTS5. https://www.sqlite.org/fts5.html | Official | I | The keyword half of hybrid search. |
| sqlite-vec. https://github.com/asg017/sqlite-vec | Project docs | I | The vector half. `vec0` tables, metadata columns, partition keys. |
| Nango docs. https://docs.nango.dev/ | Official | I | Connect sessions, syncs, webhooks, records. |
| Gmail API overview. https://developers.google.com/gmail/api/guides | Official | I | Threads, labels, scopes, what Nango wraps. |
| Resend Inbound. https://resend.com/docs/dashboard/receiving/introduction | Official | B | The forwarding path. |
| Caddy docs. https://caddyserver.com/docs/ | Official | B | Reverse proxy and automatic TLS. |
| *The Twelve-Factor App*. https://12factor.net/ | Short site | B | Config in env, logs as streams, one build many deploys: the deploy pipeline follows it. |

## Module 03: Tools

| Resource | Type | Level | Why |
|---|---|---|---|
| *The Cargo Book*. https://doc.rust-lang.org/cargo/ | Official | B | Profiles, features, workspaces, the env vars CI uses. |
| GitHub Actions docs. https://docs.github.com/en/actions | Official | B | Workflow syntax, secrets, environments, concurrency. |
| `gh` CLI manual. https://cli.github.com/manual/ | Official | B | `gh run`, `gh pr`, `gh secret`. |
| Docker multi-stage builds. https://docs.docker.com/build/building/multi-stage/ | Official | B | Why the Dockerfile has three `FROM`s. |
| Playwright docs. https://playwright.dev/docs/intro | Official | I | Traces, `webServer`, fixtures, `show-report`. |
| Vitest docs. https://vitest.dev/guide/ | Official | B | Config inside Vite, jsdom, `--run`. |
| Git worktree manual. https://git-scm.com/docs/git-worktree | Official | I | The parallel-branch workflow used here. |
| *Site Reliability Engineering* (Google), chapters on monitoring and incident response. https://sre.google/sre-book/table-of-contents/ | Free book | I | Why the daily self-test emails a person and why incidents are separate from audit. |

## Module 04: AI and email basics

| Resource | Type | Level | Why |
|---|---|---|---|
| *AI Engineering*, Chip Huyen (O'Reilly, 2025) | Book (paid) | I | The best single book on building products on foundation models: evaluation, RAG, agents, cost, latency. |
| Anthropic, "Building effective agents". https://www.anthropic.com/engineering/building-effective-agents | Article | I | Workflows versus agents, the patterns this repo follows. Cited as [A1] in `docs/agent-building-guide.md`. |
| Anthropic, "Effective context engineering for AI agents". https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents | Article | I | Block order, budgets, just-in-time retrieval. [A2]. |
| OpenAI, "A practical guide to building agents". https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf | PDF | B/I | Risk tiers and guardrails. [O]. |
| OpenRouter docs: structured outputs, provider routing, prompt caching, tool calling, usage accounting. https://openrouter.ai/docs | Official | I | Every flag in `ai/openrouter.rs` is explained here. |
| DeepSeek KV cache guide. https://api-docs.deepseek.com/guides/kv_cache | Official | I | Why stable prompt prefixes save money. |
| OWASP Top 10 for LLM Applications. https://owasp.org/www-project-top-10-for-large-language-model-applications/ | Project | I | LLM01 prompt injection, LLM06 sensitive data, with mitigations. |
| Simon Willison on prompt injection and the "lethal trifecta". https://simonwillison.net/tags/prompt-injection/ | Blog series | I | The clearest running commentary; the term used in `docs/research-2026-10.md`. |
| "Spotlighting" (Hines et al., 2024). https://arxiv.org/abs/2403.14720 | Paper | A | Datamarking, the `^`-joined words in the planner's view. |
| "CaMeL" (Debenedetti et al., 2025). https://arxiv.org/abs/2503.18813 | Paper | A | The dual-LLM, quarantined-extractor design. |
| Jay Alammar, "The Illustrated Transformer". https://jalammar.github.io/illustrated-transformer/ | Article | B | What tokens and attention are, in pictures. |
| Pinecone, "Hybrid search" and RRF explainers. https://www.pinecone.io/learn/hybrid-search-intro/ | Article | B | Why BM25 plus vectors, and how RRF merges them. |
| MIME and email headers: RFC 5322 (message format) and RFC 2045 (MIME). https://www.rfc-editor.org/rfc/rfc5322 | Standard | A | When you need the exact rule for `In-Reply-To` or `List-Unsubscribe`. |

## Module 05: Agentic architecture

| Resource | Type | Level | Why |
|---|---|---|---|
| Anthropic, "Building effective agents" (again) | Article | I | Read it twice: once before module 05, once after. |
| Google Cloud, "Choose a design pattern for your agentic AI system". https://docs.cloud.google.com/architecture/choose-design-pattern-agentic-ai-system | Guide | I | Routing, planner, critic patterns. [G]. |
| LangGraph concepts: human-in-the-loop and persistence. https://langchain-ai.github.io/langgraph/concepts/ | Docs | I | To compare the approval interrupt with `agents/approvals.rs`. |
| Temporal, "What is durable execution?". https://temporal.io/blog/what-is-durable-execution | Article | I | The idea the jobs table implements by hand. |
| Mastra docs. https://mastra.ai/docs | Docs | I | A TypeScript sibling of this design. |
| OpenAI Agents SDK docs. https://openai.github.io/openai-agents-python/ | Docs | I | Guardrails and handoffs as a reference design. |
| Claude Agent SDK overview. https://docs.anthropic.com/en/docs/agents-and-tools/claude-agent-sdk | Docs | I | Permissions and tools in a production harness. |
| "PRELUDE/CIPHER: learning user preferences from edits" (Gao et al., 2024). https://arxiv.org/abs/2404.15269 | Paper | A | Where the per-user preference idea comes from. |

## Module 06: Evals and quality

| Resource | Type | Level | Why |
|---|---|---|---|
| Hamel Husain, "Your AI product needs evals". https://hamel.dev/blog/posts/evals/ | Article | I | The most practical essay on evals: unit tests, human review, LLM judge, in that order. |
| Hamel Husain and Shreya Shankar, evals course material and "Who validates the validators?" https://arxiv.org/abs/2404.12272 | Paper and course | I/A | Calibrating judges against humans. |
| Anthropic, "Demystifying evals for AI agents". https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents | Article | I | Code graders first, pass^k, regression versus capability suites. [A3]. |
| OpenAI Evals guide. https://platform.openai.com/docs/guides/evals | Official | B/I | A vendor's framing of graders and datasets. |
| Anthropic, "Define your success criteria" and "Create strong empirical evaluations". https://docs.anthropic.com/en/docs/build-with-claude/define-success and https://docs.anthropic.com/en/docs/build-with-claude/develop-tests | Official | B | Short, concrete, grader examples. |
| Eugene Yan, "Patterns for building LLM-based systems" and "Evaluating LLM-based applications". https://eugeneyan.com/writing/llm-patterns/ | Articles | I | Evals, RAG, guardrails, caching, with references. |
| *Accelerate*, Forsgren, Humble, Kim | Book (paid) | B | Why the deploy pipeline, tests and rollbacks matter as much as the model. |

## Module 07: Alternatives

| Resource | Type | Level | Why |
|---|---|---|---|
| Postgres docs: `SELECT ... FOR UPDATE SKIP LOCKED`. https://www.postgresql.org/docs/current/sql-select.html#SQL-FOR-UPDATE-SHARE | Official | I | The queue pattern you would move to. |
| pgvector. https://github.com/pgvector/pgvector | Project | I | The vector option on Postgres. |
| Temporal docs. https://docs.temporal.io/ | Official | I/A | Deterministic workflows, activities, signals, versioning. |
| Shortwave Ghostwriter and Superhuman AI product pages | Product | B | To price and position "buy". |
| `docs/research-2026-10.md` in this repo | Internal | B | The market notes that shaped the thesis. |

## Learning habits that work for this codebase

- Read one source file a day, start to finish, with the docs open. `apps/api/src/jobs/mod.rs`,
  `agent_registry/routing.rs`, `agents/autonomy.rs`, `replies.rs` first.
- Change something small and run the nearest test. The test files are the second documentation.
- Keep a `docs/learn/notes.md` (not committed, or committed, your call) with one line per thing you learned and the
  file it lives in.
- When a doc and the code disagree, trust the code, then fix the doc in the same commit.

**Back to:** [README](README.md).
