# 07. Alternatives: other ways to build this, and the trade-offs

**Goals.** Argue, fairly, for and against the choices in this repo: Rust versus Node, SQLite versus Postgres, a
hand-built agent runtime versus a framework, a hand-built job table versus Temporal, hosted email AI products
versus building, and the RAG options. Know the signal that would make you switch each one.

**Prerequisites.** Modules 02, 05 and 06. **Reading time.** 1 hour.

A good engineer can defend the choice they did not make. Every row below has a "switch when" column. None of these
are wrong answers; they are answers to different situations.

## 1. Backend language: Rust versus Node/TypeScript

| | Rust (this repo) | Node + TypeScript |
|---|---|---|
| Fit for you | New language; the compiler is the teacher | Your daily language; the whole team can contribute on day one |
| Correctness | Exhaustive `match`, no null, typed errors, data races impossible | Good with strict TS and zod, but runtime surprises remain (undefined, thrown anything) |
| Runtime | One small binary, low memory, predictable latency, no GC pauses while 4 workers and the API share a process | Fine for this load; more memory; a worker and API are usually two processes |
| Libraries for this domain | Axum, SQLx, Tokio are excellent; email and Office parsing are thinner (hence `pdf-extract`, `calamine`, zip by hand) | Richer: `mailparser`, `googleapis`, LangChain and the OpenAI/Anthropic SDKs are first-class |
| Build and iteration | Slow first build, slower edit-compile loop; CI needs the debug-info trick | Instant reload, fast CI |
| Hiring | Harder | Easy |

**Switch when:** you need to add many integrations fast and the team is TypeScript-only; or AI SDK features
(streaming agents, provider tool runners) matter more than runtime cost. **Stay when:** one cheap VPS, one binary,
and "the compiler catches it" are the point. A middle path exists: keep the Rust API and worker, write new side
services (a Gmail add-on, a Slack bot) in Node against `docs/api.md`.

## 2. Database: SQLite versus Postgres

| | SQLite, WAL (this repo) | Postgres |
|---|---|---|
| Ops | A file. Backup is `VACUUM INTO`. Nothing else to run | A server to run, patch, back up, monitor (or pay a host) |
| Concurrency | Many readers, **one writer** at a time; `BEGIN IMMEDIATE` and `busy_timeout` make it queue, not fail | Many writers; MVCC |
| Scale | One host. The DB and the app must be on the same machine | Many app hosts, read replicas |
| Features used here | FTS5, `sqlite-vec` vectors, JSON functions, triggers, cascades, all in one transaction with your rows | `tsvector`, `pgvector`, better JSONB, LISTEN/NOTIFY for job wake-ups |
| Testing | In-memory DB per test, fast | Needs a container or a test DB |
| The job queue | Works because `UPDATE ... RETURNING` is atomic | Classic `SELECT ... FOR UPDATE SKIP LOCKED` pattern, scales to many workers |

**Switch when:** you need two or more app hosts, write throughput beyond a few hundred writes per second with
long transactions, or the write lock shows up in `tests/db_concurrency.rs`-style tests under real load. **Stay
when:** one host, tens of workspaces, and "no database server to run" is worth more than horizontal scale. The
code already keeps SQL in plain strings with `?` binds, so a move is mechanical but not free (SQLite-only syntax:
`INSERT OR`, `strftime`, vec0).

## 3. Agent frameworks versus the hand-built runtime

What this repo built by hand: a spec contract, a tool registry with risk tiers, a planner loop, a critic, approvals,
durable steps as jobs, autonomy levels. Frameworks sell the same parts.

| Framework | What it gives | What it costs | Fit here |
|---|---|---|---|
| **LangGraph** (Python, JS) | Graph of nodes, state checkpointing, `interrupt()` for human approval, time travel | Python/JS runtime; its own persistence layer; abstractions that hide prompts | Strong match for the approval-and-resume shape. Would replace `agents/case.rs` and `runner.rs`. Needs a second language or a rewrite |
| **OpenAI Agents SDK** | Agents, handoffs, guardrails, tracing, tied to OpenAI models (works with others via compatible endpoints) | Vendor lean; Python/JS | The guardrail idea maps to the critic; handoffs map to `reroute`. Model-agnostic needs (OpenRouter, no-retention routing) are a poorer fit |
| **Claude Agent SDK** | The harness behind Claude Code: tools, permissions, sessions, subagents | Anthropic models; this repo's rule is "no Anthropic defaults in production" | Great for internal developer agents; wrong for a product that must run on the cheapest capable model |
| **Mastra** (TypeScript) | Agents, workflows with suspend/resume, memory, evals, a dev UI | TS only; younger project | Closest TypeScript sibling to this design (workflows and agents on one runtime, evals built in). Worth a prototype if you ever go Node |
| **CrewAI** (Python) | Role-based multi-agent crews | Encourages many agents talking to each other, which the research brief calls hype for an MVP | Poor fit; the product needs few, non-overlapping agents |
| **Temporal** | Durable execution as a service: workflows survive crashes, sleep for months, retries, signals, versioning | A server (or cloud), SDK in Go/TS/Python/Java (Rust SDK exists but is young), new mental model | Replaces the `jobs` table, leases, backoff and `schedule_check`. Serious option when cases run for weeks and there are many of them |

**What you keep regardless:** the spec contract with `deny_unknown_fields`, routing in code, the sensitive floor,
`send_draft` as the only door, evals. Frameworks do not ship your safety rules.

**Switch when:** the runtime becomes the thing you spend most of your time on (tracing, resumability,
multi-step branching), or you hire people who already know one of these. **Stay when:** 15-step cases with one tool
per turn and a SQLite job table are enough, and owning every line of the safety path matters (it does, for email).

## 4. Durable workflows: the jobs table versus Temporal, or a Rust queue

| | `jobs` table (this repo) | Temporal | `apalis` or similar Rust crate |
|---|---|---|---|
| Durability | Row per step, lease, retry, dead letter; the case state is a JSON column | Event history replays the workflow function; sleeps, signals, child workflows | Jobs over Redis/Postgres/SQLite with retries and cron |
| Human-in-the-loop | An approval row; the case waits; a decision enqueues the next step | A signal wakes the workflow | Same as the table, by hand |
| Visibility | Jobs page, audit trail | Web UI with full history per workflow | Depends |
| Cost | Zero infrastructure | A cluster or a paid cloud | Zero to one extra store |

The jobs table works because the product has a small number of job kinds and every handler is idempotent. It stops
working when you need fan-out/fan-in across hundreds of steps, or an audit of exactly which code version ran
each step. Temporal's versioning solves that, at the price of a service and a new way of thinking (deterministic
workflow code, side effects in activities).

## 5. Build versus buy: hosted email AI

| Product | What it does well | Where it does not fit the Post Room thesis |
|---|---|---|
| **Superhuman AI** | Fast triage, instant replies, split inbox; polished personal client | Per-seat consumer pricing; one person's inbox; no shared team runtime, no HRMS, no custom agents |
| **Shortwave** | Ghostwriter learns your voice from sent mail; AI search; good Gmail client | Also personal-first; you cannot add a timesheet desk or a people directory |
| **Fyxer** | Drafts in Gmail/Outlook, meeting notes, learns from your replies, team plans | Autonomy is "draft and you send"; no earned autonomy, no routing to experts, no evals you can see |
| **Gmail Gemini, Outlook Copilot** | Free-ish with the suite, summaries and drafts inside the client | Generic; no team workflow, no approvals, no HR modules; you cannot change the rules |
| **Lindy, Relay, Zapier/n8n with HITL** | Fast to assemble flows with approvals | Each flow is hand-wired; no learned autonomy, cost per task adds up, prompt injection defences are yours to build |

The thesis of this repo is the part none of them sell: **a shared team runtime where helpers earn freedom, with
packs for a field, auditable and cheap**. If that thesis stops mattering (a single-user product, a client-first
product), buying is cheaper than building.

**Switch when:** the users want a mail *client*, not a back office; or when "drafts in my voice" alone is the
whole value. **Stay when:** the value is the shared inbox, the HRMS desks, the approval inbox, and the audit trail
a company can show its auditor.

## 6. RAG and search options

| Option | Here | Alternative | Trade |
|---|---|---|---|
| Vectors | `sqlite-vec` in the same file | pgvector, Qdrant, LanceDB, Turbopuffer | Separate store = another backup and consistency problem; sqlite-vec is young but tiny |
| Keyword | FTS5 BM25 | Elasticsearch, Meilisearch, Typesense | Operational cost; here hybrid search is one SQL file |
| Fusion | Reciprocal rank fusion in Rust | Reranker model (Cohere, bge-reranker) | A reranker improves precision for a few cents; after-v1 item 1 already plans an LLM rerank for matching |
| Chunking | 800 chars, 120 overlap, per document cap | Semantic chunking, parent-child retrieval | More recall, more tokens, more code |
| Memory | SQLite tables with FTS and a guard | Mem0, Zep, Letta | They add graph memory and decay; `README.md` notes they can replace `memory::context_for` behind the `Learner` trait if semantic recall is ever needed |
| Embeddings | `bge-m3` via OpenRouter, 1024 dims | `qwen3-embedding-8b` ($0.01/M, 32k ctx), local models | Price and language coverage; a change means re-embedding everything |

**Switch when:** search quality is the user complaint, or the corpus passes a few million chunks per workspace.
**Stay when:** search is a supporting feature and one-file atomicity is the point.

## 7. The AI provider: OpenRouter versus direct

OpenRouter gives one key, every model, `data_collection: deny`, `require_parameters`, and a fallback when a provider
is down. It costs a small margin and a hop of latency, and some features (explicit cache control, batch APIs) lag
the direct APIs. Going direct to DeepSeek, Qwen or Anthropic buys lower latency and the newest features at the
price of N keys, N request shapes and your own routing. The trait layer (`Classifier`, `Drafter`, `Planner`...)
is what makes either choice cheap to change.

## 8. A one-page decision record

When you change one of these, write a short note in `docs/` with: the signal that made you switch (a number from
the ledger, a failing test, a user complaint), the options you considered, the choice, and what would make you
switch back. The existing `docs/research-2026-10.md` and `docs/ai-engineering-review.md` are examples of the style.

## In this repo

- `docs/research-2026-10.md`: the market and architecture evidence behind the current choices.
- `docs/ai-engineering-review.md` section 8 and `docs/agent-building-guide.md` sources: the external guidance.
- `README.md` "Agent memory": the explicit "swappable" note on memory.

## Exercises

1. **Write the Postgres migration plan.** List every SQLite-only construct you can find in `apps/api/migrations/`
   and `apps/api/src/` (`vec0`, `INSERT OR`, `strftime`, `RETURNING` is fine, FTS5). Estimate the work in days.
2. **Prototype in Mastra or LangGraph (optional, one evening).** Model one case: read email, draft, approval
   interrupt, send. Compare the lines of code with `agents/runner.rs` plus `approvals.rs`. What did the framework
   give you, and what did you lose?
3. **Argue the other side.** Write ten lines for "we should have used Node", then ten lines against. Be fair.
4. **Price the buy.** For a 20-person recruiting firm, price Fyxer or Shortwave team plans for a year against the
   measured cost per email here times your estimate of their mail volume. Which is cheaper, and what does each
   not do?

## Check yourself

1. What is the single technical limit of SQLite that would force a move, and how does the code delay hitting it?
2. Which parts of this system would a framework like LangGraph replace, and which would it not?
3. Why is OpenRouter's `require_parameters` worth the extra hop?

<details>
<summary>Show answers</summary>

1. One writer at a time, and the file must live on the same host as every process that uses it. The code delays the pain with WAL mode (readers never block), `BEGIN IMMEDIATE` so writers queue instead of failing, short write transactions with no network calls inside, a 15 s busy timeout, and a job worker with bounded concurrency. You move to Postgres when you need a second app host or sustained write contention shows up in load tests.
2. It would replace the case and step persistence, the planner loop, the approval interrupt and resume, and tracing (`agents/case.rs`, `runner.rs`, parts of `approvals.rs` and the `agent_step` job). It would not replace the spec contract and its validation, routing in code, the autonomy levels and the sensitive floor, `send_draft` and `send_safety`, the injection gate, the ledger and incidents, or the golden evals. Those are product rules, and no framework ships them.
3. Without it, a provider that ignores `response_format` can return free text for a strict JSON call, and your parser fails in production at random. `require_parameters: true` routes only to providers that honour every parameter, so structured output is reliable across many models with one request shape. It is also why `parallel_tool_calls` must not be sent with it, or no provider matches (`docs/agent-building-guide.md` section 6).
</details>

**Next:** [08. Resources](08-resources.md).
