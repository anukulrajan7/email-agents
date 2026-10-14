# Learn Post Room: the curriculum

A learning path for the owner of this project. Goal: understand every part of what was built, well enough to
rebuild it, change it with confidence, and explain it to someone else. Every idea is tied to a real file in this
repo, with a small exercise and a "check yourself" question.

Written for a product engineer who knows React, Next, TypeScript, Node and MongoDB, and is new to Rust, SQLite
and AI agent systems. Sentences are short on purpose. Hinglish-friendly tone, English text.

## How to use it

Each module follows the same loop:

1. **Read** the module (20 to 60 minutes).
2. **Run** the commands in it on your machine (`make api`, `cargo test`, and so on).
3. **Do** the exercises, in order. They get harder. Stop when one takes more than an hour and come back later.
4. **Check** yourself with the three questions at the end. Answers are hidden under a "Show answers" toggle.

Do not read all modules in one sitting. One module per day is a good speed. The whole path is about two weeks of
evenings.

Where a module says `path:line`, the line number was correct on 15 Oct 2026. Lines move; the function or type
name next to it will still find the place (`grep -n "fn route" apps/api/src/agent_registry/routing.rs`).

## The path

| # | Module | What you will be able to do after | Time |
|---|---|---|---|
| 00 | [Vision](00-vision.md) | Say in one minute what Post Room is, who it is for, and what "good" means | 30 min |
| 01 | [Rust through this codebase](01-rust.md) | Read any file in `apps/api/src` and know what it does; write a small handler and a test | 3 h |
| 02 | [Architecture](02-architecture.md) | Draw the system from memory; trace one email from Gmail to a sent reply | 2 h |
| 03 | [Tools and commands](03-tools-and-commands.md) | Build, test, deploy, read logs, debug a failed deploy or e2e run | 1.5 h |
| 04 | [AI and email basics](04-ai-email-basics.md) | Explain tokens, structured output, embeddings, prompt injection, and cost per email | 2 h |
| 05 | [Agentic architecture](05-agentic-architecture.md) | Create, update, version and roll back an agent; build a timesheet agent end to end | 3 h |
| 06 | [Evals and quality](06-evals-and-quality.md) | Add a golden scenario; read a scorecard; explain the promotion gate | 2 h |
| 07 | [Alternatives](07-alternatives.md) | Argue for and against this stack versus LangGraph, Temporal, Postgres, Node | 1 h |
| 08 | [Resources](08-resources.md) | Know where to go deeper on each topic | 15 min |

Order matters for 00 to 03. After that, 04 to 06 build on each other. 07 and 08 can be read at any time.

## Prerequisites

- Rust stable installed (`rustup`), Node 24, pnpm 12. See `README.md` at the repo root.
- A local `.env` (`cp .env.example .env`). No API keys are needed for most exercises: without an OpenRouter key
  the app uses rules and templates (`apps/api/src/ai/fallback.rs`, `apps/api/src/ai/mock.rs`).
- About 10 GB of free disk for Rust build output (`apps/api/target`).

## Progress checklist

Copy this into your notes and tick as you go.

```
[ ] 00 Vision: I can explain "an AI helper earns freedom" and the four autonomy levels.
[ ] 01 Rust: I wrote one new Axum route with a test, and it passed clippy.
[ ] 02 Architecture: I traced one test email through the jobs table and read its audit rows.
[ ] 03 Tools: I ran the full test suite, built the Docker image locally, and read a deploy log.
[ ] 04 AI basics: I estimated the cost of one email on paper and checked it against the AI usage page.
[ ] 05 Agents: I built the timesheet agent in the designer, saw the dry run, saved it, and rolled back a version.
[ ] 06 Evals: I added a golden scenario and the offline suite passed with it.
[ ] 07 Alternatives: I wrote a one-page note on when I would switch to Postgres or Temporal.
[ ] 08 Resources: I picked one book and one course and started.
```

## Where the facts come from

These modules summarise and link to the deeper docs. When they disagree, the code wins, then these docs:

- `docs/architecture.md`: the system, components, security, deploy.
- `docs/features.md`: everything a person can do today.
- `docs/agent-building-guide.md` and `docs/ai-engineering-review.md`: the AI rules and the cost numbers.
- `docs/evals.md`, `docs/selftest.md`: the eval suites and the daily self-test.
- `docs/status-2026-10-15.md`, `docs/roadmap.md`: what is live, what is next.
- `docs/api.md`: the REST contract.
