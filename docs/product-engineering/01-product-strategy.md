# 01 — Product Strategy

**Summary.** Post Room is a personal, proactive email agent for teams that live in Gmail: it reads mail, labels it, drafts replies in the owner's voice, runs fixed workflows (timesheets, leave, candidates), and only sends when a guard says it is safe. The owner's north star is "Muse for email". This document checks that claim against the real competitors (verified 14 Oct 2026), states what Post Room should do better than them, what it should not build, and the next three milestones. The product is live at `mail.anukulraj.tech`; what is live versus planned is listed in `docs/status-2026-10-15.md`.

Prices below are quoted as reported on 14 Oct 2026. Where only secondary sources were reachable, the row says so. Reporting currency for our own costs is INR in `06-cost-optimization.md`; competitor prices stay in USD because that is how they are sold.

---

## 1. What we are, in one paragraph

A workspace connects one or more Gmail inboxes (through Nango) or forwards mail to an inbound address (Resend). Every new email is filtered by rules, understood once by a small model, routed to a named helper (Meera for personal mail, one expert per pack), and either handled by a fixed workflow or a bounded planner loop. Drafts are built from a verified context pack; anything unsupported becomes `[NEEDS: …]`. Sends pass deterministic guards and a 10-minute hold. Helpers earn autonomy levels 0 to 3 from evidence (safety suite, approvals, edit distance) and lose a level automatically on incidents. Everything is one Rust process on one VPS with one SQLite file.

The code backs this up: `apps/api/src/agent_registry/routing.rs` (routing), `agents/runner.rs` (planner loop), `agents/autonomy.rs` (levels), `send_safety.rs` and `replies.rs` (guards and hold), `evals/gate.rs` (promotion checklist).

---

## 2. Competitive landscape (verified)

### 2.1 Categories first

"Muse" and "Manus" are not in the same category as each other, nor as an email assistant. Confusing them leads to building the wrong thing.

| Category | Products | What the user buys |
|---|---|---|
| General personal agent | **Meta Muse** (launched 8 Sep 2026) | An agent that acts across email, calendar, browsing and purchases, with its own sandbox and approval guard |
| General task agent (cloud computer) | **Manus** | A credit-metered agent that does research, builds slides, browses, runs code |
| AI email client | **Superhuman**, **Shortwave**, **Fyxer** | A better inbox: triage, drafts in your voice, follow-up detection |
| Platform-native AI | **Gmail with Gemini**, **Outlook with Copilot** | Summaries and smart replies inside the mail client people already pay for |
| No-code agent builders | **Lindy**, Zapier agents, n8n | Build your own automations with an LLM step and integrations |
| HR software with an AI assistant | BambooHR, Lattice, Culture Amp | HRIS first; AI is a feature |

Post Room sits between "AI email client" and "no-code agent builder", with an HR pack that borrows from the last row. No product found in this search is an email agent built for HR or staffing teams.

### 2.2 Product by product

| Product | Core capability | Agent execution | Integrations | UX | Pricing (as reported) | Strengths | Weaknesses | Gap we can use |
|---|---|---|---|---|---|---|---|---|
| **Meta Muse** | Personal agent: reads mail and calendar, fills forms, buys things, remembers you, proposes tasks | Runs in a cloud VM you can watch; a separate **Sentinel** agent must approve internet access and asks the user; per-app read vs send scopes; audit trail | Gmail, Google Calendar, OpenTable, Facebook, Instagram, Peloton, Plaid at launch | Own app + WhatsApp; US only, 18+ | Free tier plus **$20** and **$100**/month (secondary sources; Meta names not confirmed) | Security design (isolated VM, credential vault, scopes, audit) and transparency are the selling point | Brand trust ("hate the owners"), US only, consumer not team, model reportedly trails Opus 5 on agent benchmarks; a reported address leak passed its approval | Teams, India, shared inboxes, HR workflows, cost transparency. We copy the *pattern* (guard agent, scopes, audit), not the product |
| **Manus** | General agent that researches, builds slides and docs, browses, runs code in a cloud computer | Credit-metered autonomous runs; "Agent Mode"; Wide Research fan-out | Slack, files, browser; not an inbox product | Chat + task view | Free (300 daily credits, Lite model), **Pro from $20/mo for 4,000 credits**, Team from ~$20/seat (2-seat min); a task can cost 20 to 9,000 credits | Breadth; strong for one-off knowledge work | Unpredictable per-task cost; no durable inbox state; no team approval model | Predictable per-email cost; always-on inbox agent rather than per-task sessions |
| **Superhuman** | Fast email client; auto-labels, auto-drafts, auto-reminders, Ask AI | Rules + model features inside the client | Gmail, Outlook, CRMs | Keyboard-first desktop/mobile | Starter **$25 to $30**/user/mo; Business **$33 to $40** | Polish, speed, follow-up detection from your own sent mail | Per-user price is high for Indian SMEs; no workflows, no approval model, no team agents | Team workflows and sends with an audit trail at a fraction of the seat price |
| **Fyxer** | AI executive assistant: one category per email, drafts in your voice, meeting notes | Model-driven triage and drafting, learns from moves | Gmail, Outlook, calendar | Lives inside your inbox | **$22.50 to $50**/mo | Learns from behaviour; never drafts for automated senders | Single-user; no workflows; no autonomy ladder | Same learning idea, plus team-level rules and HR records |
| **Shortwave** | AI email client with gists, Ghostwriter drafts from your own sent mail, search | Cheap model for gists at scale, big model on demand | Gmail | Client | Pro **$14 to $18**/mo, Business ~$24/seat | Cost-aware model routing | Gmail only, individual focus | Our tiered routing already matches; add team scope |
| **Lindy** | No-code AI agents ("Lindies") for email, scheduling, CRM | Credit-metered agent runs with HITL steps | Hundreds via integrations | Builder UI | Plus **$29.99**, Pro **$99.99**, Max **$199.99**/seat with shared credit pools; extra credits $10 per 1,000 (Sept 2026 sources; plans changed during 2026) | Breadth of integrations | Credit anxiety; generic drafts; the user builds the agent | A **designer that builds the agent from one sentence** and a safety suite that gates it (we have this) |
| **Gmail Gemini / Outlook Copilot** | Thread summaries, smart replies, "help me write" | Inline features | Native | Native | Bundled with Workspace / M365 | Zero adoption friction | No team routing, no workflow, no autonomy, no audit of sends | Act on mail, not just summarise it |

### 2.3 What the best ones share

- Understand the **thread**, not the message: open asks, commitments, who is waiting on whom.
- Draft in the **user's own voice** from their own sent mail.
- Detect **follow-ups** from the last outgoing message.
- Never draft for **automated senders**.
- Make cost **predictable** (seat or free tier), or users complain about credits.

Post Room already has the second, fourth and fifth. The first is half built (`ai/understand.rs` v2 in shadow; no `thread_state` table yet). The third exists only as the `remind_no_reply` automation.

---

## 3. Our strategy

### 3.1 The problem we should solve better than anyone

**"My team's inbox does real work, and I cannot see or control what the AI did."**

Every competitor either does nothing risky (summaries) or does risky things with a thin approval layer (Muse's reported address leak, Lindy's user-built agents). Post Room's edge is the combination that none of them ship together:

1. A per-person agent that acts on mail (not a summariser).
2. Autonomy that is **earned from evidence** and **lost on incident** (`evals/gate.rs`, `agents/autonomy.rs`).
3. Deterministic guards on every send (`send_safety.rs`: recipient, leak, gaps, duplicates), not model opinion.
4. An audit trail with content hashes on approvals (`audit.rs`, `agents/approvals.rs`).
5. A cost per email the admin can see (`ai_ledger.rs`, AI health page).

### 3.2 Differentiation, in order of defensibility

| Rank | Differentiator | Why it is hard to copy | Where it is |
|---|---|---|---|
| 1 | **Autonomy ladder with automatic demotion** | Needs the eval suite, metrics rollup, incident system and approval history wired together; takes months of product judgement | `evals/gate.rs`, `incidents.rs`, `metrics/` |
| 2 | **Designer: one sentence → versioned, validated, dry-run agent** | Competitors make the user build; we generate, validate against a strict schema (`agent_registry/spec.rs` rejects unknown fields), repair, dry-run and gate | `designer/` |
| 3 | **Pack model (HR/staffing as one pack)** | Domain depth (people directory, submissions, timesheets, portal reconciliation, matching) that generic agents lack | `team/`, `staffing/`, `matching.rs` |
| 4 | **Cost per email under ₹0.10** | Rules-first bulk filter, single-flight analysis, tiered models, daily budgets; most competitors meter credits instead | `bulk.rs`, `ai/routing.rs`, `ai_ledger.rs` |
| 5 | Verified context pack and `[NEEDS]` gaps | Good engineering, but reproducible | `learning/context_pack.rs` |

### 3.3 Features we should NOT build

- **Fully autonomous sending by default.** Level 3 exists; keep it behind an explicit admin grant and the hold. Muse's leak is the cautionary tale.
- **Hundreds of integrations.** Gmail (Nango), forwarding (Resend) and SMTP are enough until a paying customer asks. Each integration is a new OAuth review and a new attack surface.
- **Multi-agent swarms.** The planner loop is already a cost sink (4 to 5 calls per routine mail). Fewer, cheaper, more deterministic steps win; see `05-agentic-system-design.md`.
- **Voice as the primary UI.** Push-to-talk exists; do not invest further until usage shows demand.
- **Meeting bots, calendar booking, purchases.** Out of scope; Muse does these, we do not need to.
- **A second database or a queue service.** SQLite plus the jobs table handles the load at the scale modelled in `06-cost-optimization.md`.
- **Microsoft 365** until a pilot customer needs it.

### 3.4 MVP scope (what "done" means for a paying pilot)

Most of this is live. The items marked ◻ are the gaps a stranger would hit.

- ✔ Sign-up with verified email, invite-only, workspace, packs.
- ✔ Gmail connect (admin connects), forwarding, bulk filter, labels, Gmail-style reader.
- ✔ Drafts with `[NEEDS]`, send guards, hold, undo, no duplicate sends.
- ✔ Helpers with autonomy levels, approvals inbox, audit trail, AI health page.
- ✔ Designer with dry run; golden evals offline (89/89 pass, `docs/evals/scorecard.md`).
- ◻ **Members connect their own Gmail** (consent flow; today an admin connects each mailbox).
- ◻ **Live golden baseline** with real models recorded as the bar (offline only today).
- ◻ **Thread model** (`thread_state`, commitments, waiting-on) so follow-ups and drafts use thread facts.
- ◻ **Understanding v2 on** once shadow agreement is at least 95%.
- ◻ A restore drill for the SQLite backup.

### 3.5 Pricing stance (proposal, not implemented)

Charge per **mailbox**, not per credit, and show the admin the measured cost per email. At the modelled cost of roughly ₹0.05 to ₹0.10 per email (`06-cost-optimization.md`), a mailbox receiving 60 emails a working day costs under ₹200 a month to run; a ₹999 to ₹1,999 per mailbox per month price leaves margin for support and infra while undercutting Superhuman and Lindy seat prices several times over. Offer the HR pack as an add-on. Verify with three pilot conversations before building billing.

---

## 4. Next three product milestones

Each milestone is small enough to ship from this codebase without new infrastructure. Detailed tasks, dependencies and acceptance criteria are in `07-engineering-mistakes-and-improvements.md` (Phase table) and the roadmap section of `02-current-architecture.md`.

### Milestone A — "Trust it on real mail" (stabilise)

- Run the live golden suite against production models and record pass rate and cost per email in `docs/evals/scorecard.md`.
- Turn understanding v2 on when the shadow page says ready; delete the classify/analyze path afterwards.
- Members connect their own Gmail with a consent screen; personal Meera works only on that mailbox.
- Ship the top fixes from `07` Phase 1 (planner diet, deterministic retry classes, caching).
- **Exit criteria:** a new workspace onboarded by a stranger reaches level 1 for Meera within a week with zero high-severity incidents, and 7-day cost per email on the AI health page is under ₹0.10.

### Milestone B — "It knows the thread" (improve)

- `thread_state` and `contact_cards` tables; outbound understanding call; hourly overdue pass (designed in `docs/ai-engineering-review.md` §3).
- Follow-up drafts from commitments and `waiting_on`, replacing the blind `remind_no_reply`.
- Routine replies as a workflow, planner only on escalation.
- **Exit criteria:** follow-up precision measured on 50 real threads at or above 0.8; routine-reply path uses one model call plus one judge.

### Milestone C — "Pilot ready for staffing teams" (differentiate)

- LLM rerank for referral-to-requirement matching; lift the five-attachment cap.
- Ask Meera answers HR numbers (the metric functions exist, the assistant does not call them).
- Billing per mailbox, invoice via a payment link; usage page shows cost against plan.
- **Exit criteria:** two paying pilots, each with at least one helper at level 2.

---

## 5. Risks to the strategy

| Risk | Signal to watch | Mitigation |
|---|---|---|
| Google OAuth verification for restricted Gmail scopes | Nango dev environment, unverified app warnings | Start the verification early; forwarding path as fallback |
| Model quality on real Indian business mail is unproven | Offline keyword baseline is 58% (`docs/evals/scorecard.md`) | Milestone A live baseline before any sales |
| One VPS, one SQLite file | Disk, backup restore never drilled | Restore drill in CI; see `07` |
| Owner cannot yet defend the architecture | 165 commits in two days, almost all AI-written | `08-personal-learning-roadmap.md` |
| Credit-free pricing could be gamed by a mailbox with huge volume | Daily per-workspace budget already exists | Keep `AI_DAILY_BUDGET_USD`, add per-mailbox cap |

---

## 6. Sources (verified 14 Oct 2026)

Meta Muse: [Quartz launch coverage](https://qz.com/meta-muse-ai-agent-email-travel-purchases-090926) · [eesel review](https://www.eesel.ai/blog/meta-muse-agent-review) · [Yahoo Finance on tiers](https://finance.yahoo.com/technology/ai/articles/metas-muse-arrives-three-pricing-183437528.html) · [Fieldy](https://www.fieldy.ai/blog/meta-muse). Pricing is from secondary coverage; muse.ai itself was not fetched.

Manus: [help center pricing article](https://help.manus.im/en/articles/11711111-what-is-the-current-membership-pricing-for-manus) · [docs plans page](https://manus.im/docs/introduction/plans.md) · [eesel](https://eesel.ai/blog/manus-ai-pricing) · [nocode.mba](https://www.nocode.mba/articles/manus-ai-pricing). The official pricing page did not render for extraction.

Email assistants: [Missive roundup](https://missiveapp.com/blog/ai-email-assistant) · [Gmelius](https://gmelius.com/blog/best-ai-assistants-for-email) · [alfred_](https://get-alfred.ai/blog/best-ai-email-assistants) · [InboxPilot](https://www.inboxpilot.co/blog/ai-email-assistant-for-teams) (vendor blogs; prices cross-checked across at least two).

Lindy: [coworker.ai](https://coworker.ai/blog/lindy-ai-pricing) · [jetadmin](https://www.jetadmin.io/blog/lindy-pricing/amp/) · [hackceleration](https://hackceleration.com/labs/lindy-pricing).

Design guidance already cited in the repo: [Anthropic, Building effective agents](https://www.anthropic.com/engineering/building-effective-agents) · [OpenAI, A practical guide to building agents](https://cdn.openai.com/business-guides-and-resources/a-practical-guide-to-building-agents.pdf) · `docs/research-2026-10.md`.

---

## 7. Next steps

1. Read `02-current-architecture.md` to see what exists, then `07` for the ranked problems.
2. Decide on the pricing stance in §3.5 with three customer conversations.
3. Start Milestone A with the live golden baseline (needs `OPENROUTER_API_KEY`, budget about $2).
