# Product engineering audit (14 Oct 2026)

Eight documents written from the repository at commit `0e368c8`, with tests run the same day (`cargo test` 826 passed, 3 ignored; `vitest` 291 passed; `pnpm typecheck` clean). The same content is a multi-tab Claude Doc for reading and commenting: https://claude.ai/code/artifact/19b9948f-28d3-4c09-83e2-270eac127b89

| # | Document | What it answers |
|---|---|---|
| 01 | [Product strategy](01-product-strategy.md) | Verified competitors, differentiation, what not to build, pricing stance, next three milestones |
| 02 | [Current architecture](02-current-architecture.md) | Verified inventory: implemented, partial, missing; how components talk |
| 03 | [Rust backend explained](03-rust-backend-explained.md) | Router, extractors, SQLite rules, job queue, traced requests, exercises |
| 04 | [JavaScript and React explained](04-javascript-react-explained.md) | Routing, API client, TanStack Query, rendering, exercises |
| 05 | [Agentic system design](05-agentic-system-design.md) | Current AI pipeline evaluated, alternatives compared, target architecture, production controls |
| 06 | [Cost optimization](06-cost-optimization.md) | Verified prices, cost per email in INR, 100 / 1,000 / 10,000 users, ranked savings |
| 07 | [Engineering mistakes and improvements](07-engineering-mistakes-and-improvements.md) | Ranked defects with scenarios, fixes, tests; three-phase roadmap |
| 08 | [Personal learning roadmap](08-personal-learning-roadmap.md) | 18 lessons tied to files and defects, six-week pace |
| 09 | [Learn: start here](09-learn-start-here.md) | The learning path for someone new to Rust and AI agents |
| 10 | [L1 Rust basics](10-rust-basics.md) | Twelve Rust ideas through our code |
| 11 | [L2 Rust intermediate and advanced](11-rust-advanced.md) | Lifetimes, traits, async, tokio, axum, sqlx, macros, unsafe |
| 12 | [L3 Rust principles](12-rust-principles.md) | Fourteen principles: where we follow and break them |
| 13 | [L4 AI agent development](13-ai-agent-development.md) | Fifteen levels from tokens to durable, guarded agents |
| 14 | [L5 Glossary](14-glossary.md) | Every term, in plain words, with where it lives |
| 15 | [Test speed plan](15-test-speed-plan.md) | Measured: 12.4 min local, 25-50 min deploy; six-step fix |
| 16 | [Pipeline and security check](16-pipeline-security-check.md) | CI, Docker and repo check: two high findings, no leaked secrets |

Start with 07 for action, 08 for study, 02 for orientation. When these disagree with the code, the code wins; update the document.
