# Task prompts: paste one per session, in order

Each block below is a complete prompt for a fresh Claude Code session in `/Users/anukul/Desktop/email-agents`.
Paste **one** at a time; when it reports done, tick the box in `docs/TASKS.md` and paste the next.

Every prompt starts with the same rules block, so a fresh session knows how to work:

```
RULES (Post Room): one task, on main, no worktrees, no subagents that build in parallel. Read docs/TASKS.md
(the row for this task) and the detail doc it points to. Write or extend the test first, then fix. Run only the
touched tests (`cargo test --test api -- <module>::` or `cargo test --lib <module>`), then
`cargo clippy --all-targets -- -D warnings`; web: `pnpm typecheck` + the touched vitest file. Build with
`CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0`, cargo at ~/.cargo/bin/cargo, git at /usr/bin/git. Never two cargo
builds at once. Commit with a clear message (numbers in it for speed/cost work), tick the task in docs/TASKS.md.
Don't push unless the prompt says so. Push only as anukulrajan7:
T=$(gh auth token -u anukulrajan7); git -c credential.helper= push "https://x-access-token:${T}@github.com/anukulrajan7/email-agents.git" main
(mask the token). Back up the prod DB before any deploy that has migrations. Report: what changed, tests run, commit hash.
```

---

## Section 0: finish the current round

**0.5 HR pack + live test pack**
```
<RULES>
Task 0.5 + 0.6. Check production (VPS root@187.127.166.111, /docker/email-agents, DB data/email_ai.db, read it
read-only with python3 sqlite3): is the deploy of main live (image tag = main's sha), migration 57, health ok?
The HR pack (staffing) is OFF in workspace ws_01a136852f757593a9efb0bd1be54bd3. ASK ME whether to turn it on
before doing anything. If yes, turn it on through the app's API or UI path (not a raw DB write), then confirm
Ananya, Rohan, Sneha, Arjun, Aditya are active at level 3 and Vikram is at 1. Then send the live test pack:
copy scripts/test-pack/ to the VPS, run `python3 -I send.py --to anukulraj2004@gmail.com --dry-run`, show me the
list, and after I say go run it for real. Wait ~10 min, run `python3 -I check.py runs/<run>.json`, and show the
scorecard. Explain each FAIL in one line.
```

**0.7 Cleanup**
```
<RULES>
Task 0.7. List git worktrees (/usr/bin/git worktree list) and the build folders ~/.cache/postroom-target*.
For each worktree, show whether its branch is fully merged into main (git branch --merged main). Remove merged
worktrees and branches, and delete all but one target dir (keep ~/.cache/postroom-target-verify). Show the disk
space before and after. Don't delete anything unmerged; list it and ask me.
```

## Section 1: foundation, fast tests and builds

**A5 nextest (measure first)**
```
<RULES>
Task A5. Tests are now one binary (`cargo test --test api`, ~66 s run, ~116 s full on an M4). Measure
`cargo nextest run` against `cargo test` (wall time, 2 runs each). Adopt nextest only if it's ≥20% faster; if so,
add `make test` and use it in .github/workflows/deploy.yml. Otherwise write the numbers in
docs/learn/03-tools-and-commands.md and stop. Note the purge timing test is #[ignore] and runs alone as its own step.
```

**A6 CI pipeline**
```
<RULES>
Task A6 (docs/product-engineering/15-test-speed-plan.md, 16-pipeline-security-check.md). Restructure
.github/workflows/deploy.yml: clippy and tests as parallel jobs; the docker image build in parallel; deploy needs all
three plus e2e (make e2e a required gate, task B1). Add cargo-chef to the Dockerfile so dependency layers are cached.
Keep CARGO_PROFILE_*_DEBUG=0. Measure CI time on 2 runs before and after (gh run list/view). Targets: Rust job
< 10 min on a cache hit, deploy < 15 min. Push to deploy only after I confirm.
```

**S-1 pycache**
```
<RULES>
Task S-1. Remove the committed scripts/test-pack/__pycache__/ with `git rm -r --cached`, add __pycache__/ and *.pyc
to .gitignore, commit.
```

## Section 2: foundation, structure pass

**R1 module moves**
```
<RULES>
Task R1. Restructure apps/api/src feature-first, following the map in docs/TASKS.md section 2: platform/ mail/
understanding/ replies/ agents/ learning/ hr/ ai/ assistant/ automations/ alerts/ evals/ metrics/ jobs/. Do ONE
domain per commit, starting with replies/ (replies.rs, send_safety.rs, gap_resolver.rs, repair.rs, signoff.rs,
owner_said.rs, reply_settings.rs). Use `git mv` (keeps history) plus `pub use` re-exports in lib.rs so old paths
still compile. Zero behaviour change. After each domain: `cargo test --test api` + `--lib` + clippy, all green, and
compile time not slower (measure). Show me the final tree before starting the next domain.
```

**R2 remove re-exports**
```
<RULES>
Task R2. After R1, remove the compatibility `pub use` re-exports, one domain per commit, and update all imports
(crate code and tests). Full `cargo test --test api` + clippy green after each.
```

**R3 split big files**
```
<RULES>
Task R3. Split the god files, one per commit, no behaviour change: agents/tools.rs (one file per tool under
agents/tools/), jobs/handlers.rs (each feature registers its own jobs in its jobs.rs; jobs/ keeps only the queue
and a registry table), ai/openrouter.rs (client/{chat, embeddings, vision, audio, pricing}.rs),
staffing/timesheet_records.rs, sources/nango.rs. Target: no file over ~600 lines. Tests + clippy green each time.
```

**R4 SQL out of route handlers**
```
<RULES>
Task R4. 23 of the 32 files in http/ (or the feature api.rs files after R1) run SQL directly. Move every query into
that feature's store.rs and have route handlers call feature functions only. One feature per commit. Every query
keeps its workspace_id filter; tests/tenant_isolation.rs must pass.
```

**R5 tests mirror domains**
```
<RULES>
Task R5. Tests are one binary (tests/api.rs with #[path] modules). Move the test files into tests/it/{mail, replies,
agents, hr, ...} matching the src domains, with git mv; update api.rs and the guard test
every_test_file_is_part_of_this_binary. Same test count before and after (show both).
```

**R6 web structure**
```
<RULES>
Task R6. apps/web: merge features/agents and features/agents-hub into features/agents/{cases, team, builder}; fold
features/drafts into features/inbox; replace the 12 feature types.ts files + api/types.ts with one API types
module (generated later in P2.4; for now one hand-written file). git mv, no behaviour change; typecheck + all
vitest green; run the e2e specs that cover agents and inbox.
```

## Section 3: Phase 1, stabilise

**B1 e2e gate** (skip if A6 already did it)
```
<RULES>
Task B1. Deploy runs even when the e2e workflow is red. Make deploy depend on e2e passing (same workflow, or
workflow_run with a success check). Prove it with a deliberately failing e2e on a branch (don't merge that).
```

**P1.1 lease heartbeat (critical)**
```
<RULES>
Task P1.1 (C1 in docs/product-engineering/07). jobs/mod.rs LEASE_MS is 120 s and only claim() sets locked_until,
so a long job can be claimed twice. Write the test first: 1 s lease, a 3 s handler, assert it runs exactly once.
Then add a heartbeat in jobs/worker.rs process() that extends locked_until every lease/4, fenced by locked_by +
attempts, and stops when the job ends.
```

**P1.2 panic capture**
```
<RULES>
Task P1.2 (C2). jobs/worker.rs drops the JoinHandle, so panics are never recorded. Test first: a handler that
panics → the job is dead-lettered with last_error starting "panic: ". Await the handle; on panic call jobs::fail
(permanent) and on_dead.
```

**P1.3 budget choke point**
```
<RULES>
Task P1.3 (C3). One budget check for every AI spend path: gate GENERATE_DRAFT, EXECUTE_WORKFLOW and index jobs in
jobs/handlers.rs deferred_until_opened; add draft to AUTOMATIC_TASKS; meter Jev (ai/jev.rs) and the judge; bind
spec.budget in agents/case.rs create; return usage from draft_email and read_email tools; ledger prices in
agents/planner.rs Usage::from_chat. Tests: over-budget drafts defer; Jev calls are in the ledger; case spent_usd
within 10% of the ledger (tests/ai_usage.rs, tests/agents.rs).
```

**P1.4 injection fencing**
```
<RULES>
Task P1.4 (C4). Fence subject, sender, goal and state facts in agents/runner.rs render_prompt, agents/critic.rs and
the agents/tools.rs read_email output. Make ai/understand.rs injection_signals set injection_suspected, and have
agents/autonomy.rs blocker read it. Narrow ai/mod.rs prepare_text redaction to key: value shapes. Add golden
scenario inj_subject_line to apps/api/src/evals/scenarios.json; it must pass. Check "I can't reset my password"
survives understanding.
```

**P1.5 atomic approvals**
```
<RULES>
Task P1.5 (C5). agents/approvals.rs decide isn't atomic. Use states pending → approved → applied with a sweeper
that finishes approved-but-not-applied ones; add origin_key dedupe for notify_owner and send_message. Test with
crash injection between approve and apply: the action applies exactly once.
```

**P1.6 web error boundary + admin guard**
```
<RULES>
Task P1.6. apps/web: an ErrorBoundary around <Outlet/> in src/app/AppShell.tsx and around Ask Meera; a RequireAdmin
route guard for packs, jobs, usage, ai-health. Vitest: a throwing child shows the fallback. Playwright: an operator
visiting /jobs sees an explanation, not a blank page.
```

**P1.7 worker concurrency**
```
<RULES>
Task P1.7. Worker concurrency 8 (config), and move the maintenance pass out of jobs/worker.rs into its own task.
Test with AI_MOCK: a 500-email burst keeps queue age under 60 s.
```

**P1.8 retry classes**
```
<RULES>
Task P1.8 (H10). Schema and validation failures from drafting are retried as transient and re-billed up to 5x.
Make them one repair turn, then permanent. Test: such failures take at most 2 attempts in the AI ledger.
```

**P1.9 docs and ledger truth**
```
<RULES>
Task P1.9. Fix docs to match code: docs/architecture.md level-3 wording and scenario count (95);
docs/status-2026-10-15.md; ai_ledger V4 Pro price 0.4106/0.8213 (check current OpenRouter pricing first). Add a
test that pins the price table if one doesn't exist.
```

**P1.10 live golden row** (skip if docs/evals/scorecard.md already has a live row for the current sha)
```
<RULES>
Task P1.10. Run the live golden once: export OPENROUTER_API_KEY from the VPS .env without printing it, then
`GOLDEN_WRITE_SCORECARD=1 cargo test --test api golden_live -- --ignored --nocapture` with GOLDEN_BUDGET_USD=2.
Commit the scorecard row. Report the pass rate per agent and the cost per email.
```

**F1 planner loop**
```
<RULES>
Task F1. Live golden (docs/evals/scorecard.md): Meera 5/34, Ishaan 1/7, Arjun 3/8, Kabir 3/6. They hit the
15-step cap and end failed/handoff instead of approval; fixed-workflow desks pass 100%. Find why the planner
(agents/planner.rs, agents/runner.rs) doesn't stop: e.g. no "done" signal after draft_email, repeated reads,
tool results it doesn't recognise. Fix the stop condition (a draft ready for approval = finish). Prove it offline
with a scripted test that reproduces the loop, then run the live golden filtered to those four agents
(GOLDEN_FILTER) within $1. Target ≥80% each, 0 safety failures.
```

**F2 timesheet reader live**
```
<RULES>
Task F2. The timesheet AI reader (apps/api/src/ai/timesheet_reader.rs) was only tested with a scripted reader. Run
it against real models on the test-pack sheets (scripts/test-pack/scenarios.py builds xlsx/pdf samples: a
mismatched total, a forwarded PDF, a pasted table). Write an #[ignore] live test like golden_live that reads them
and checks employee code, period and total. Report accuracy and cost.
```

## Section 4: Phase 2, improve

Paste these after section 3. Each one: `<RULES>` + "Task P2.x from docs/TASKS.md section 4; read its row and
docs/product-engineering/07 section 3; propose the plan in 5 lines and wait for my OK before coding." The
tasks: P2.1 understanding v2, P2.2 workflow-first inbox, P2.3 Member extractor, P2.4 web response validation,
P2.5 thread model, P2.6 prompt caching, P2.7 planner diet, P2.8 SSE, P2.9 FTS + N+1, P2.10 members' own Gmail.

## Section 5: small items

```
<RULES>
Task T<n> from docs/TASKS.md section 5 (replace <n>). Read its row, do it, test it, commit. If it is marked Owner, ask me first.
```

## Section 7: Phase 3

```
<RULES>
Task P3.<x><letter> from docs/TASKS.md section 7 (e.g. P3.1a). Ask me before starting the first subtask of each
P3 item. Then one subtask per commit.
```
