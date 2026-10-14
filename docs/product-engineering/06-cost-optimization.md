# 06 — Cost Optimization and Unit Economics

**Summary.** At today's configuration one inbound email costs about **₹0.11** in model calls with agents on (₹0.05 for labelling only), and the fixed infrastructure is under **₹2,000 a month**. Model spend is the only cost that scales with users; Nango connections are the second. At 1,000 active mailboxes the total is roughly **₹1.5 lakh a month, about ₹145 per mailbox**, and the planned changes bring that to about ₹85. The first bottleneck at scale is not money but the worker: with `JOB_WORKER_CONCURRENCY=2` the pipeline can process roughly 11,000 emails a day, which is enough for about 400 mailboxes. This document shows every number's source, the formulas, the sensitivity, and a ranked plan.

All INR figures use **₹95 per USD** (an assumption; observed rates in September 2026 were 94.8 to 96). Verified prices are marked **V**; estimates **E**.

---

## 1. Verified prices (14 Oct 2026)

### 1.1 Models, from the OpenRouter models API (`GET https://openrouter.ai/api/v1/models`) — V

USD per 1M tokens. The ledger's own table is in `apps/api/src/ai_ledger.rs:21-30`; differences are noted.

| Model | Input | Output | Cache read | Ledger table | Used for (production, `deploy.yml:138-147`) |
|---|---|---|---|---|---|
| `deepseek/deepseek-v4-flash` | 0.091 | 0.182 | 0.0182 | 0.091 / 0.182 ✔ | Draft, understand, learn, suggest, balanced planner tier |
| `deepseek/deepseek-v4-pro` | 0.4106 | 0.8213 | 0.0342 | 0.40 / 0.80 (stale by 3%) | Premium draft, escalation, planner, designer |
| `qwen/qwen3.8-flash` | 0.15 | 0.47 | 0.016 | ✔ | Classifier, analyst, judge, critic (GitHub var; `.env.example` default) |
| `moonshotai/kimi-k2.6` | 0.65 | 3.41 | 0.15 | ✔ | `OPENROUTER_MODEL_ID` fallback (GitHub var) |
| `qwen/qwen3-vl-30b-a3b-instruct` | 0.15 | 0.60 | 0 | ✔ | Vision |
| `anthropic/claude-haiku-5.5` | 0.10 | 0.50 | 0.01 | ✔ | Not in production |
| `qwen/qwen3.7-flash` | 0.03 | 0.13 | 0.006 | not listed | Candidate cheaper classifier |
| `z-ai/glm-5.3-flash` | 0.036 | 0.90 | 0.0108 | not listed | Candidate |

The ledger prefers the provider's `usage.cost` when present (`ai_ledger.rs:56-62`) and only falls back to this table, so the stale V4 Pro entry matters only for calls where OpenRouter omits cost.

### 1.2 Providers — V unless marked

| Provider | Plan | Price | Notes |
|---|---|---|---|
| Nango | Free | $0, 10 connections, 10 h compute, 10 GB | Hard cap |
| Nango | Pay-as-you-go | $50/mo minimum, includes $50 credit; **$0.29 per connection/month**, $0.72/compute hour, $0.50/GB | Enterprise "as low as $0.01/connection" |
| Resend | Free | 3,000 emails/mo, 100/day | **Inbound counts against the quota** |
| Resend | Pro | $20 for 50k; $35 for 100k; $0.90 per extra 1k | |
| Resend | Scale | $90 (100k), $160 (200k), $350 (500k), $650 (1M), $825 (1.5M) | |
| Hostinger VPS KVM 2 (2 vCPU, 8 GB, 100 GB NVMe) | — | **E** ₹799/mo introductory, ₹999 to ₹1,199 at renewal (third-party price trackers; hostinger.in not fetched) | Current host |
| GitHub Actions (private repo) | Free tier | 2,000 min/mo included; $0.008/min after | **E** usage not measured |
| SMTP (Hostinger mail) | Included with hosting | ₹0 marginal | Verification codes, alerts |
| Google OAuth verification for restricted Gmail scopes | One-time | **E** CASA security assessment typically $500 to $4,500 via an authorised lab; larger assessments more | Required before onboarding strangers' Gmail |

---

## 2. Where the money goes per email

Source: `docs/ai-engineering-review.md` §1 (per-100-email assumptions: 35% bulk, 40% need a reply, 30% start an agent case, 25% get a sent draft, 25% have attachments, 10% images; "balanced" agent tier). These are **E**: estimated call counts times verified prices, not production measurements. The AI health page shows the measured number.

| Path | Today (`UNDERSTAND_V2=off`) | After v2 on | Target after all fixes |
|---|---|---|---|
| Labelling only | $0.00053 = **₹0.050** | $0.00025 = ₹0.024 | $0.0002 = ₹0.019 |
| Agentic (agents on, balanced) | $0.00119 = **₹0.113** | $0.00091 = ₹0.086 | $0.00046 = ₹0.044 |
| Agentic, "best" tier | $0.0026 = ₹0.247 | $0.0023 = ₹0.219 | — |

What the ₹0.113 is made of (today):

| Component | Share | Why |
|---|---|---|
| Planner loop (4.5 calls per case on 30% of mail) | 29% | Routine replies still run a planner loop |
| Classify + analyze + duplicates | 28% | Two calls read the same email; v2 merges them |
| Memory extraction (incoming + reply) | 12% | Reply memory moved from Kimi to V4 Flash already |
| Send-safety judge | 8% | One judge call per approval or send |
| Critic (2 per case) | 7% | |
| Drafts (agent + workflow) | 8% | |
| Embeddings, vision, judges | 8% | |

**Formula.** `cost_per_email = Σ_paths (calls_per_email × (tokens_in × price_in + tokens_out × price_out) / 1e6)`. Each row's tokens are in the review's §0 table (for example understanding: ~1,600 in, ~300 out on V4 Flash = $0.0002).

---

## 3. Cost at 100, 1,000 and 10,000 active users

### 3.1 Workload assumptions — E

- An **active user** is one connected mailbox.
- **40 inbound emails per working day, 22 working days** = 880 emails per mailbox per month. (Sensitivity in §3.3.)
- Agents are on by default (level 3 behind the hold, per `docs/status-2026-10-15.md`), so the agentic figure applies.
- 20% of mailboxes use forwarding (Resend); 80% connect Gmail (Nango). Each Gmail mailbox is one Nango connection.
- Ask Meera: 15 commands per user per month at about $0.002 each (planner on V4 Pro, ~2,000 tokens) = $0.03 per user.
- Storage: ~20 KB per email including text and insights (bodies are capped at 200,000 chars; attachments store text only).

### 3.2 Monthly totals (INR, ₹95/USD)

| Line | Formula | 100 users | 1,000 users | 10,000 users |
|---|---|---|---|---|
| Emails per month | users × 880 | 88,000 | 880,000 | 8,800,000 |
| **Model calls, today** | emails × $0.00119 | ₹9,950 | ₹99,500 | ₹9,95,000 |
| Model calls, v2 on | emails × $0.00091 | ₹7,600 | ₹76,000 | ₹7,61,000 |
| Model calls, target | emails × $0.00046 | ₹3,850 | ₹38,500 | ₹3,85,000 |
| Ask Meera | users × $0.03 | ₹285 | ₹2,850 | ₹28,500 |
| Nango connections | max($50, 0.8 × users × $0.29) | ₹4,750 | ₹22,040 | ₹2,20,400 (negotiate enterprise) |
| Resend (20% forwarding) | plan covering 0.2 × emails | ₹1,900 (Pro 50k) | ₹15,200 (Scale 200k) | ₹90,000 (1.5M + overage) **E** |
| VPS | see §4 | ₹1,199 (KVM 2) | ₹3,000 (KVM 4 class) **E** | ₹10,000+ or a managed DB **E** |
| CI, domain, SMTP | **E** | ₹500 | ₹1,000 | ₹2,000 |
| **Total, today** | | **₹18,600** | **₹1,43,600** | **₹13,46,000** |
| **Per user, today** | | **₹186** | **₹144** | **₹135** |
| **Per user, target** | | ₹125 | ₹83 | ₹73 |

Observations:

- Below 1,000 users, fixed costs (Nango minimum, VPS) dominate the per-user number. Above it, model spend is 70% of the bill and is the only lever worth engineering.
- Nango at $0.29 per connection is 20% of the per-user cost at every scale. It buys OAuth, token refresh, sync and a proxy. Replacing it with a direct Gmail integration saves ₹27 per user per month but costs the OAuth verification work and token-refresh code; defer until past 2,000 users.
- Storage at 10,000 users is 8.8M emails × 20 KB ≈ 176 GB per month. With 30-day retention the SQLite file is 170 GB, and the nightly `VACUUM INTO` copy (`backup.rs`) doubles that. This, not query speed, is what forces a change at that scale: move bodies to object storage or move to Postgres with a managed backup. SQLite is fine through 1,000 users (about 17 GB).

### 3.3 Sensitivity (1,000 users, today's per-email cost)

| Variable | Low | Base | High | Monthly model cost |
|---|---|---|---|---|
| Emails per mailbox per day | 20 | 40 | 80 | ₹49,750 / ₹99,500 / ₹1,99,000 |
| Bulk share filed by rules | 25% | 35% | 45% | +14% / base / −14% of labelling spend |
| Share of mail starting an agent case | 10% | 30% | 50% | ₹63,000 / ₹99,500 / ₹1,36,000 |
| Agent tier | balanced | balanced | best | ₹99,500 / ₹99,500 / ₹2,17,000 |
| USD/INR | 90 | 95 | 100 | −5% / base / +5% |

The two variables that matter are email volume (linear) and how much mail enters the planner loop. The latter is a product decision: routine replies should not use the planner (§5, item 2).

---

## 4. Server and throughput, not just rupees

**Throughput today.** `JOB_WORKER_CONCURRENCY=2` (`.env.example`, `config.rs`). A new email triggers 5 to 6 jobs; the model-bound ones take 3 to 8 s each, so an email occupies a worker slot for roughly 15 s of wall time. Capacity ≈ 2 slots × 86,400 s / 15 s ≈ **11,500 emails a day**, about **400 mailboxes** at 40 a day each, before queues grow. The jobs are I/O-bound (waiting on OpenRouter), so raising concurrency to 8 or 16 costs almost no CPU and lifts capacity 4 to 8 times. That is a one-line change, and the first thing to do at 300+ mailboxes. The claim query and SQLite can take far more: a claim is one indexed `UPDATE`.

**Database writes.** At 1,000 users: 880k emails × ~15 writes ≈ 13M writes a month ≈ 5 per second average, with bursts at mail peaks. SQLite in WAL with `synchronous=NORMAL` on NVMe sustains thousands of small transactions per second with one writer. Fine. At 10,000 users it is 50 per second average and storage becomes the issue (§3.2).

**Memory and CPU.** The process is small (Rust, no JIT). Spikes come from attachment parsing (`spawn_blocking`) and Argon2 (capped at 4 concurrent). KVM 2 (2 vCPU, 8 GB) is enough through about 1,000 mailboxes if concurrency is raised. Beyond that, a 4 to 8 vCPU VPS (₹2,500 to ₹5,000) is the next step, not a second server, because SQLite wants one host.

**Logs.** Plain-text tracing to the container log. No cost today; at 1,000 users expect a few GB a month; rotate with Docker's `max-size` option.

---

## 5. Optimisation plan, ranked by rupees saved per hour of work

Savings are per email at 1,000 users (880k emails/month), **E**. "Validation" is how to prove it with the ledger (`GET /workspaces/{ws}/ai-usage`, AI health page) before and after.

| # | Change | Saving per email | Monthly at 1,000 users | Effort | Risk | Validation |
|---|---|---|---|---|---|---|
| 1 | **Switch `UNDERSTAND_V2=on`** once shadow agreement is at least 95% (`GET …/understanding-shadow`) | $0.00028 | ₹23,400 | Hours (flag flip, then delete the old path) | Medium: one call is a single point of failure; rules fallback exists | Label agreement on the shadow page; cost per email on AI health; `tests/understanding_v2.rs` |
| 2 | **Routine replies as a workflow**, planner only on escalation (complaint, legal, 3+ asks, reopened thread) | $0.00035 | ₹29,300 | Days (`agents/triggers.rs`, `runner.rs`, templates; the fixed-step path already exists in `agents/workflow.rs`) | Medium: less flexibility on odd mail | Golden offline + live; planner call count per email in the ledger drops from 4.5 to <1 |
| 3 | **Prompt caching with stable prefixes** and `session_id` on DeepSeek paths | $0.0001 to 0.0002 | ₹8,800 to ₹17,600 | Days (prompt reordering in `ai/openrouter.rs`, `agents/runner.rs::render_prompt`) | Low; verify `cached_tokens` per provider because `data_collection: deny` can route elsewhere | `ai_calls.cached_tokens` share on V4 Flash paths above 40% |
| 4 | **Planner context diet**: typed understanding fields instead of raw bodies, `max_tokens` 300, per-spec tool allowlist | $0.0001 per step | ₹13,000 | Days (`agents/tools.rs`, `planner.rs`) | Medium: planner may lose nuance | Tokens in per `agent_plan` call; golden live pass rate unchanged |
| 5 | **Judge only on automatic sends**; a person's send gets deterministic checks plus a warning | $0.00007 | ₹6,200 | Hours (`send_safety.rs`) | Low | Judge calls per email |
| 6 | **Consolidate the learner**: one call per approved draft, skip when edit distance < 0.03 | $0.00003 | ₹2,600 | Hours | Low | `learn` calls per email |
| 7 | **Deterministic retry classes**: "link in draft" and "too long" are repaired once, not retried 5 times | Worst-case cap | Protects against spikes | Hours (`ai/openrouter.rs` draft validation) | Low | `ai_calls.attempt` distribution |
| 8 | **Cheaper classifier**: blind test `qwen/qwen3.7-flash` ($0.03/$0.13) or V4 Flash against `qwen3.8-flash` on 200 labelled emails | up to $0.00006 while classify exists; moot after #1 | ₹5,000 | Hours | Medium: accuracy | `tests/golden_understanding.rs` plus a live 200-email set |
| 9 | **Raise `JOB_WORKER_CONCURRENCY` to 8** | ₹0; capacity ×4 | — | Minutes | Low; watch OpenRouter rate limits | Queue depth and oldest-pending age |
| 10 | **Reduce polling** on the web (SSE for list and badges) | Server CPU, not rupees | — | Days | Low | Requests per minute per tab |
| 11 | **Update the ledger price table** (V4 Pro 0.4106/0.8213) and add the candidate models | Accuracy only | — | Minutes | None | `tests/ai_usage.rs` |
| 12 | **Thread model** (`thread_state`, outbound understanding) | +$0.00003 cost, better drafts | — | Week | Medium | Follow-up precision |

Items 1 to 4 together take the agentic figure from ₹0.113 to about ₹0.05 per email, which is the target column in §2. Items 1, 5, 6, 7, 9 and 11 are each under a day and need no architectural change.

### Tradeoffs to state plainly

- **Cheaper models lower quality on hard mail.** The design already routes the top 5% by confidence to V4 Pro. Keep that escalation and measure edit distance on drafts when a tier changes.
- **Caching makes prompts rigid.** A stable prefix means the email goes last and workspace blocks must not change per call. That is also better for attention in small models, so the tradeoff is mostly discipline.
- **Fewer planner turns means less flexibility.** The escalation rule in item 2 keeps the planner for the unpredictable 5 to 10%.
- **Hard budgets cause silent deferral.** `AI_DAILY_BUDGET_USD` (default $2 per workspace) marks deferred jobs `succeeded` (`jobs/handlers.rs:119`). A workspace receiving 2,000 emails in a day would stop labelling at about 1,700 under today's per-email cost. Raise the default with the plan tier, and show "deferred" rather than "succeeded" in the jobs page.

---

## 6. What to measure every week

| Metric | Where | Target |
|---|---|---|
| Cost per email, 7-day, automatic tasks | AI health page, `ai_ledger::usage` | < ₹0.05 after items 1 to 4 |
| Calls per email by task | `ai_calls` grouped by task / inbound count | understand ≤ 1, agent_plan < 1, judge < 0.5 |
| Cached token share on V4 Flash | `ai_calls.cached_tokens` | > 40% |
| Escalation rate | `classify` calls answered by V4 Pro | < 10% |
| Schema failure rate | `ai_calls.schema_ok = 0` | < 2% |
| Oldest pending job age | `jobs` table | < 60 s |
| Model spend vs plan revenue per mailbox | ledger vs billing (future) | < 15% |

---

## 7. Sources

OpenRouter models API (prices, 14 Oct 2026): https://openrouter.ai/api/v1/models · [OpenRouter prompt caching](https://openrouter.ai/docs/features/prompt-caching) · [Nango pricing](https://www.nango.dev/pricing) · [Resend pricing](https://resend.com/pricing) · Hostinger VPS India price trackers: [Zoutons, Aug 2026](https://zoutons.com/news/hostinger-vps-hosting-price-india-2026), [Findstack](https://findstack.com/resources/best-vps-hosting-india) · USD/INR observed Sept 2026: [dollarrupee.in](https://dollarrupee.in/) · Call-path token estimates: `docs/ai-engineering-review.md` §0 and §1.

---

## 8. Next steps

1. Flip items 9 and 11 today (minutes each).
2. Check the shadow page; if ready, do item 1 and delete the classify/analyze path.
3. Schedule items 2 to 4 as Phase 1 and 2 work in `07-engineering-mistakes-and-improvements.md`.
4. Record the measured cost per email from the AI health page in this document monthly, replacing the estimates.
