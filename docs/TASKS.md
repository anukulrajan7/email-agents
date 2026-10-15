# Post Room task list

One list, worked top to bottom, **one task at a time on `main`**. It combines the audit (`docs/product-engineering/`, IDs
like C1, H10, P1.3) with findings from the 14–15 Oct build session. Details for audit items are in
`docs/product-engineering/07-engineering-mistakes-and-improvements.md` (C/H/L ids) and `15-test-speed-plan.md` (A ids).

## How to work a task

1. Write or extend the test first (it should fail).
2. Make the fix.
3. Run **that test file** (`cargo test --test <file>` or `cargo test --lib <module>`) and
   `cargo clippy --all-targets -- -D warnings`. Web: `pnpm typecheck` and the touched Vitest file.
4. Commit, with the before/after numbers in the message when the task is about speed or cost.
5. Tick the box here, then start the next task.

Rules: `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`; never two cargo builds at once; never merge while a build runs;
one full suite only before a push; `caffeinate -i` for long runs; push only as `anukulrajan7`.

Size: **S** = hours, **M** = days, **L** = a week or more. **Owner** = needs the owner's decision before starting.

---

## 0. Finish the current round (this session)

| | Task | Notes |
|---|---|---|
| [ ] 0.1 | Merge `wp-ts-fix` (timesheet review fixes) | overlap-replace, over-labelling, retryable model errors, model-number checks, "submitted" rule, CSV injection, UI dates |
| [ ] 0.2 | Merge `wp-evals` (95 scenarios, per-agent scorecard) | replaces the early `docs/evals/scorecard.md` |
| [ ] 0.3 | Merge `wp-fasttests` (one test binary, template DB, safety suite run once and in parallel, CI cache) | covers A1–A4; record before/after |
| [ ] 0.4 | One full suite, then push and deploy | migrations 0055, 0056 (+0057 if added) |
| [ ] 0.5 | Owner: HR pack on or off for the live test | it is **off** now, so HR mail goes to Meera |
| [ ] 0.6 | Send the live test pack and record the scorecard | `scripts/test-pack/send.py` on the VPS, then `check.py` |
| [ ] 0.7 | Clean up worktrees and the extra build folders (~19 GB in `~/.cache/postroom-target*`) | keep one target dir |

## 1. Speed (rest of Part A)

| | Task | Size | Done when |
|---|---|---|---|
| [ ] A5 | `cargo nextest` locally (`make test`) and in CI, only if it's faster than one binary | S | measured |
| [ ] A6 | CI: clippy and tests as parallel jobs; image build in parallel; `cargo-chef` in the Dockerfile; deploy waits for both; e2e is a required gate | M | CI Rust job < 10 min on a cache hit, deploy < 15 min |
| [ ] S-1 | Remove committed `scripts/test-pack/__pycache__/`, add `__pycache__/` to `.gitignore` | S | |

Targets: local `cargo test` < 4 min after a one-line change (12.4 min on 15 Oct).

## 2. Structure pass (before the fixes, so fixes land in the new layout)

The module map the owner was shown on 15 Oct: feature-first folders `platform/ mail/ understanding/ replies/ agents/
learning/ hr/ ai/ assistant/ automations/ alerts/ evals/ metrics/ jobs/`. Each feature has `mod.rs` (public API),
`api.rs` (routes), `store.rs` (all SQL) and `jobs.rs`. Route handlers contain no SQL.

| | Task | Size | Done when |
|---|---|---|---|
| [ ] R1 | Pure moves with `git mv` plus `pub use` re-exports, one domain per commit | M | full suite green, no behaviour change, compile time not slower |
| [ ] R2 | Remove the re-exports; update imports | S | |
| [ ] R3 | Split god files: `agents/tools.rs` (one file per tool), `jobs/handlers.rs` (each feature registers its own jobs), `ai/openrouter.rs` (chat, embeddings, vision, audio, pricing), `staffing/timesheet_records.rs`, `sources/nango.rs` | M | no file over ~600 lines |
| [ ] R4 | Move SQL out of route handlers into each feature's `store.rs` (23 of 32 http files run SQL) | M | |
| [ ] R5 | Tests mirror the domains: `tests/it/{mail, replies, agents, hr, ...}` | S | |
| [ ] R6 | Web: merge `features/agents` and `agents-hub` into `features/agents/{cases, team, builder}`; fold `drafts` into `inbox`; one generated API types file | M | typecheck and Vitest green |

## 3. Phase 1: stabilise (fix what breaks or overspends)

| | Task | Size | Done when |
|---|---|---|---|
| [ ] B1 | e2e red on main but deploy still runs: make e2e a required gate (connect-forwarding test already fixed in `wp-agentfix`) | S | deploy blocked by a red e2e |
| [ ] P1.1 | **C1 Critical** lease heartbeat: extend `locked_until` every 30 s while a job runs (fenced by `locked_by` + attempts) | S | 1 s lease with a 3 s handler runs once |
| [ ] P1.2 | **C2** panic capture: await the job's JoinHandle; a panic dead-letters with `last_error "panic: ..."` | S | test |
| [ ] P1.3 | **C3** one budget choke point: gate draft / workflow / index jobs; meter Jev and the judge; `spec.budget` binds; ledger prices in planner usage | M | over-budget drafts defer; case `spent_usd` within 10% of the ledger |
| [ ] P1.4 | **C4** fence subject, sender, goal and facts in prompts; `injection_signals` feed the autonomy blocker; narrow `prepare_text` redaction | S | new golden `inj_subject_line` passes |
| [ ] P1.5 | **C5** atomic approval apply (pending → approved → applied, with a sweeper); `origin_key` dedupe for `notify_owner` and `send_message` | M | crash-injection test applies once |
| [ ] P1.6 | Web ErrorBoundary around `<Outlet/>` and Ask Meera; `RequireAdmin` guard for packs, jobs, usage, AI health | S | Vitest + Playwright |
| [ ] P1.7 | Worker concurrency 8; maintenance in its own task | S | queue age < 60 s in a 500-mail burst (AI_MOCK) |
| [ ] P1.8 | **H10** deterministic failures (schema, validation) take at most 2 attempts | S | ledger shows ≤ 2 |
| [ ] P1.9 | Docs and ledger truth: `docs/architecture.md` level-3 wording; scenario count; `docs/evals.md` scenarios path is `apps/api/src/evals/scenarios.json`; V4 Pro price 0.4106/0.8213 | S | |
| [ ] P1.10 | Live golden baseline recorded in `docs/evals/scorecard.md` (a live row with pass rate and cost per email) | S | done by `wp-evals` if its row is present |
| [ ] F1 | **Planner loop (live eval, 15 Oct):** Meera 15%, Ishaan 14%, Arjun 38%, Kabir 50% hit the 15-step cap and end `failed`/`handoff` instead of `approval`. Fixed-workflow desks pass 100%. Fix the stop condition first, then P2.2/P2.7 | M | those four ≥ 80% on the live golden, 0 safety failures |
| [ ] F2 | Timesheets: the AI reader was only tested with a scripted reader; run it on real messy sheets (the test pack has xlsx, pdf and forwarded cases) | S | live check passes |

## 4. Phase 2: improve (cheaper, faster, safer by design)

| | Task | Size | Depends |
|---|---|---|---|
| [ ] P2.1 | Understanding v2 on, then delete the v1 classify/analyze path (needs ≥ 95% shadow agreement over ≥ 100 emails) | S→M | shadow data |
| [ ] P2.2 | Workflow-first inbox for lead, support and sales; planner only on escalation | M | P1.3, F1 |
| [ ] P2.3 | `Member` extractor replacing 159 manual `require_role` calls | M | R4 |
| [ ] P2.4 | Web response validation (generated types, or zod at `api/client.ts`) | M | R6 |
| [ ] P2.5 | Thread model: `thread_state`, contact cards, outbound understanding, hourly overdue pass | L | P2.1 |
| [ ] P2.6 | Prompt caching with stable prefixes (DeepSeek `session_id`) | M | P2.2 |
| [ ] P2.7 | Planner diet: typed tool results, `max_tokens` 300, 6–8 tools per spec | M | P2.2 |
| [ ] P2.8 | SSE for the message list and badges instead of polling | M | |
| [ ] P2.9 | FTS for inbox search; N+1 fixes (`messaging/teams.rs:22`, `messaging/watch.rs:717`, `assistant/history.rs:171`); reuse `memory_ids` in `send_safety::gather` | S | |
| [ ] P2.10 | Members connect their own Gmail (consent flow) | L | OAuth app |

Cost target across P2.1, P2.2, P2.6 and P2.7: about ₹0.11 → ₹0.05 per email.

## 5. Small items from the build session

| | Task | Size |
|---|---|---|
| [ ] T1 | Timesheets: Excel export (CSV exists) | S |
| [ ] T2 | Timesheets: Rohan's `create_review` step doesn't fire (the legacy review item is used); decide on one path and delete the other | S |
| [ ] T3 | Owner: archive the builder-made "Timesheet Analysis Agent" (Rohan covers it now) | S |
| [ ] T4 | Tuned HR agents keep their old `max_autonomy` in the saved spec; offer a one-click "update to the new default" | S |
| [ ] T5 | Nisha's `scheduling` label isn't in the personal pack (she triggers on keywords there); add the label or document it | S |
| [ ] T6 | Owner: pack turned off → its agents show locked. Keep, or hide them? | S |
| [ ] T7 | Existing agents keep old emoji avatars; offer the 3D defaults for untouched ones | S |
| [ ] T8 | Approving after a 24 h message watch closed never reports back | S |
| [ ] T9 | Gap resolver doesn't use knowledge search for gaps | S |
| [ ] T10 | Candidate watch: no team-marked client list | S |
| [ ] T11 | Web lint: eslint missing in the agent environment; make `pnpm lint` run in CI | S |
| [ ] T12 | Run the full e2e suite (only single specs were run lately) | S |

## 6. Owner decisions (don't start alone)

- [ ] D1 Pin GitHub Actions to commit SHAs, and stop deploying as `root@VPS` (`16-pipeline-security-check.md`).
- [ ] D2 `ALLOW_TEST_EMAILS=true` is set in production. Keep or remove?
- [ ] D3 Kavya was merged into Arjun. Split again?

## 7. Phase 3: differentiate (check with the owner before each)

| | Task | Size |
|---|---|---|
| [ ] P3.1 | Autonomy ladder explicit: first-run confirmation of the starting level, recorded in audit | M |
| [ ] P3.2 | Designer v2: test against a chosen email, show cost per email before save | M |
| [ ] P3.3 | LLM rerank for matching; lift the five-attachment cap | M |
| [ ] P3.4 | Ask Meera answers HR numbers with citations | S |
| [ ] P3.5 | Billing per mailbox with usage against plan | L |
| [ ] P3.6 | Backup restore drill and migration rehearsal in CI | S |
