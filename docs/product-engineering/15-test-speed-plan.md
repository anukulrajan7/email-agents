# 15 — Test speed plan

**Summary.** Measured on 15 Oct 2026 on the owner's M4 Mac: a full `cargo test` after a code change takes **12.4 minutes**. That is about 5 minutes compiling and linking 63 test programs, then 7.5 minutes running them one after another. On GitHub Actions the Rust step took **11 to 33 minutes** and the whole deploy **25 to 50 minutes**. Several worktrees compiling their own copies in parallel on one laptop is how a run passed an hour. Six changes, in order, should bring the local suite to about 3 to 4 minutes and the deploy to about 15. Nothing below is implemented yet.

## What was measured

| Measurement | Value | Source |
|---|---|---|
| Local `cargo test`, wall time after merges changed the crate | 743.9 s (12.4 min) | `/usr/bin/time`, 15 Oct |
| Sum of suite run times | 449.7 s | per-suite `finished in` |
| Compile + link (difference) | ~294 s | derived |
| CI deploy, last run | 50 min (Rust lint + tests 33 min, image 10.5 min) | GitHub run 38594535204 |
| CI deploy, earlier run | 24 min (Rust lint + tests 11 min, image 9.5 min) | GitHub run 38545148191 |

Slowest suites:

| Suite | Tests | Seconds | Why |
|---|---|---|---|
| `tests/experts.rs` | 20 | 129.0 | One test runs the full 49-scenario safety suite for 5 templates = 245 scenario runs |
| `tests/workflow_gate.rs` | 6 | 79.0 | Three tests run the safety suite = 147 scenario runs |
| `tests/metrics_evals.rs` | 3 | 19.0 | An in-product eval run executes the safety suite |
| `tests/send_safety.rs` | 25 | 18.7 | Many full-pipeline sends |
| `tests/golden_offline.rs` | 2 | 16.7 | The 49 scenarios once (intended) |
| `tests/autonomy_levels.rs` | 26 | 15.4 | Full pipeline per test |
| All other 57 suites | ~770 | ~172 | |

## Root causes

1. **The safety suite runs about 490 times per `cargo test`.** `evals::safety::run_scenario_with` (`src/evals/safety.rs:299`) creates a temp directory, a new SQLite file, runs every migration plus agent seeding, and starts a mock Gmail server, for each of 49 scenarios, sequentially. Four suites call it, which is about 244 s, more than half of all run time.
2. **63 test programs, linked one by one and run one by one.** Each file in `tests/` is its own binary containing the whole crate. Cargo runs binaries sequentially, so the slow ones block the rest. CI comments already note that ~55 binaries "exhaust the runner disk at link time".
3. **Every test builds a fresh database and runs all migrations.** About 820 tests times 45 migrations.
4. **CI does everything in one serial job:** clippy (a separate full compile), then tests, then web, then the image build, which compiles a release build from scratch because `COPY apps/api/ ./` invalidates the dependency layer on every change.
5. **Parallel worktrees on one laptop.** Each worktree had its own `target/` (the memory notes record 150 GB of disk), so each compiled everything from scratch at the same time. Already fixed by the one-agent rule.

## The plan, in order

| Step | Change | Expected effect | Effort | Risk |
|---|---|---|---|---|
| 1 | Run the 49 safety scenarios concurrently inside `run_for_spec` (bounded, e.g. 8 at a time) and build them from one pre-migrated template database copied per scenario | 244 s to ~25 s | Small (one file) | Low; scenarios already use separate files |
| 2 | Keep one test that runs the full suite per template; mark the per-template variants in `experts.rs` and `workflow_gate.rs` to share one run or check only the changed rules | Removes ~300 scenario runs | Small | Low |
| 3 | Pre-migrated template database for every integration test (`tests/common`): migrate once per process, copy the file per test | ~1 to 2 min off run time | Medium | Low |
| 4 | Merge `tests/*.rs` into one binary (`tests/it/main.rs` with one module per old file) | Compile + link ~5 min to ~1.5 min; tests run in parallel inside one binary | Medium (mechanical) | Low; per-file statics need a check |
| 5 | Use `cargo nextest` locally and in CI | All tests run in parallel with per-test timing and retries | Small | Low |
| 6 | CI: clippy and tests as parallel jobs; image build starts in parallel and deploy waits for both; `cargo-chef` in the Dockerfile so dependencies cache; e2e becomes a required gate | Deploy 25-50 min to ~15 min | Medium | Low |

Local habits that help today, with no code change:

```bash
cd apps/api
export CARGO_PROFILE_DEV_DEBUG=0          # what CI already does; smaller, faster links
cargo test --lib                           # 409 unit tests in ~6 s
cargo test --test routing                  # only the file you touched
cargo test --test agents -- approval       # only tests whose name matches
```

## Acceptance

- Local `cargo test` after a one-line change under 4 minutes on the M4.
- CI Rust job under 10 minutes on a cache hit; deploy under 15 minutes.
- No test removed; the safety invariants still run in CI at least once per spec.
